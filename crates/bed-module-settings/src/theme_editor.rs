//! Per-view theme drafts; editing does not change preferences or write files.
use bed_settings::{Settings, theme::ThemeDraft};
use dear_imgui_rs::{ColorDisplayMode, TreeNodeFlags, Ui, sys};
use std::ffi::{CStr, CString};

const SYNTAX: [&str; 14] = [
    "Text",
    "Comment",
    "Keyword",
    "String",
    "Number",
    "Function",
    "Type",
    "Variable",
    "Parameter",
    "Property",
    "Constant",
    "Operator",
    "Punctuation",
    "Special",
];
const ANSI: [&str; 16] = [
    "Black",
    "Red",
    "Green",
    "Yellow",
    "Blue",
    "Magenta",
    "Cyan",
    "White",
    "Bright Black",
    "Bright Red",
    "Bright Green",
    "Bright Yellow",
    "Bright Blue",
    "Bright Magenta",
    "Bright Cyan",
    "Bright White",
];

fn color(ui: &Ui, label: &str, value: &mut [f32; 4]) -> bool {
    let mut rgb = [value[0], value[1], value[2]];
    if ui
        .color_edit3_config(label, &mut rgb)
        .display_mode(ColorDisplayMode::Hex)
        .build()
    {
        *value = [rgb[0], rgb[1], rgb[2], 1.0];
        true
    } else {
        false
    }
}

fn load(draft: &mut ThemeDraft, settings: &Settings, selection: &str, clone: bool) {
    match settings.load_theme(selection) {
        Ok(mut theme) => {
            if clone {
                theme.name.push_str(" Copy");
            }
            draft.load(theme, (!clone).then(|| selection.to_owned()));
        }
        Err(error) => draft.error = Some(error.to_string()),
    }
}

pub(crate) fn save(draft: &mut ThemeDraft, settings: &mut Settings, apply: bool) {
    let Some(theme) = &draft.palette else {
        return;
    };
    match settings.save_custom_theme(draft.destination.as_deref(), &theme.to_json()) {
        Ok(selection) => {
            draft.destination = Some(selection.clone());
            draft.dirty = false;
            draft.error = None;
            draft.message = Some(format!("Saved {selection}"));
            if apply && let Err(error) = settings.select_theme(&selection) {
                draft.error = Some(format!("Theme saved, but could not apply it: {error}"));
            }
        }
        Err(error) => draft.error = Some(error.to_string()),
    }
}

fn choose(ui: &Ui, draft: &mut ThemeDraft, settings: &Settings) {
    ui.text_wrapped("Open a custom theme to edit, or clone a theme to make your own.");
    if let Some(theme) = &draft.palette {
        if ui.button(format!("Continue editing {}", theme.name)) {
            draft.editing = true;
        }
        ui.spacing();
    }
    if draft.source.is_empty() {
        draft.source = settings.settings["theme"]
            .as_str()
            .unwrap_or("tokyo")
            .to_owned();
    }
    let themes = settings.list_themes();
    ui.spacing();
    ui.text("Edit a custom theme");
    if themes.iter().any(|t| t.0.starts_with("themes/")) {
        if let Some(_combo) = ui.begin_combo("Custom theme", "Choose a custom theme") {
            for (selection, name) in themes.iter().filter(|t| t.0.starts_with("themes/")) {
                let _id = ui.push_id(selection);
                if ui.selectable(name) {
                    load(draft, settings, selection, false);
                }
            }
        }
    } else {
        ui.text_disabled("No custom themes saved yet.");
    }
    ui.spacing();
    ui.separator();
    ui.spacing();
    ui.text("Clone an existing theme");
    let source_name = themes
        .iter()
        .find(|t| t.0 == draft.source)
        .map_or("Choose a theme", |t| t.1.as_str());
    if let Some(_combo) = ui.begin_combo("Clone from", source_name) {
        for (selection, name) in &themes {
            let _id = ui.push_id(selection);
            if ui
                .selectable_config(name)
                .selected(*selection == draft.source)
                .build()
            {
                draft.source = selection.clone();
            }
        }
    }
    if ui.button("Clone Theme") {
        load(draft, settings, &draft.source.clone(), true);
    }
    ui.spacing();
    ui.separator();
    if ui.collapsing_header("Paste Theme JSON", TreeNodeFlags::empty()) {
        ui.input_text_multiline(
            "##theme-json",
            &mut draft.json_input,
            [
                ui.content_region_avail()[0].max(1.0),
                ui.text_line_height() * 8.0,
            ],
        )
        .build();
        if ui.button("Paste from Clipboard") {
            let text = ui.with_bound_context(|| unsafe {
                let text = sys::igGetClipboardText();
                (!text.is_null()).then(|| CStr::from_ptr(text).to_string_lossy().into_owned())
            });
            if let Some(text) = text {
                draft.json_input = text;
            }
        }
        ui.same_line();
        if ui.button("Import JSON") {
            if let Err(error) = draft.import_json() {
                draft.error = Some(error.to_string());
            }
        }
    }
    if let Some(error) = &draft.error {
        ui.text_wrapped(error);
    }
}

pub(crate) fn draw(ui: &Ui, draft: &mut ThemeDraft, settings: &mut Settings) {
    ui.text("Theme Editor");
    ui.separator();
    if !draft.editing {
        choose(ui, draft, settings);
        return;
    }
    if ui.button("Back to Themes") {
        draft.editing = false;
        return;
    }
    ui.spacing();
    ui.text_wrapped("Click a color swatch for a picker, or enter a hex value. Changes are kept in this draft until you save.");
    if let Some(theme) = &mut draft.palette {
        ui.separator();
        let mut changed = ui.input_text("Name", &mut theme.name).build();
        let mut appearance = usize::from(theme.light);
        if ui.combo_simple_string("Appearance", &mut appearance, &["Dark", "Light"]) {
            theme.light = appearance == 1;
            changed = true;
        }
        if ui.collapsing_header("UI Colors", TreeNodeFlags::DEFAULT_OPEN) {
            changed |= color(ui, "Background", &mut theme.background);
            changed |= color(ui, "Window Background", &mut theme.window_background);
            if !draft.separate_tab_bar {
                theme.tab_bar = theme.background;
            }
            changed |= color(ui, "Surface", &mut theme.surface);
            changed |= color(ui, "Foreground", &mut theme.foreground);
            changed |= color(ui, "Accent", &mut theme.accent);
            changed |= color(ui, "Selection", &mut theme.selection);
            if ui.checkbox("Separate tab bar color", &mut draft.separate_tab_bar) {
                if !draft.separate_tab_bar {
                    theme.tab_bar = theme.background;
                }
                changed = true;
            }
            if draft.separate_tab_bar {
                changed |= color(ui, "Tab Bar", &mut theme.tab_bar);
            }
        }
        if ui.collapsing_header("Syntax Colors", TreeNodeFlags::empty()) {
            let _id = ui.push_id("syntax");
            for (label, value) in SYNTAX.iter().zip(&mut theme.syntax.slots) {
                changed |= color(ui, label, value);
            }
        }
        if ui.collapsing_header("Terminal Colors", TreeNodeFlags::empty()) {
            let _id = ui.push_id("terminal");
            for (label, value) in ANSI.iter().zip(&mut theme.terminal) {
                changed |= color(ui, label, value);
            }
        }
        if changed {
            draft.dirty = true;
            draft.message = None;
        }
        if draft.dirty {
            ui.text_disabled("Unsaved changes");
        }
        if let Some(message) = &draft.message {
            ui.text_wrapped(message);
        }
        if let Some(error) = &draft.error {
            ui.text_wrapped(error);
        }
        if ui.button("Copy Theme JSON") {
            let text =
                CString::new(serde_json::to_string_pretty(&theme.to_json()).unwrap()).unwrap();
            ui.with_bound_context(|| unsafe {
                sys::igSetClipboardText(text.as_ptr());
            });
        }
        if ui.button("Save Theme") {
            save(draft, settings, false);
        }
        ui.same_line();
        if ui.button("Save & Apply") {
            save(draft, settings, true);
        }
    } else if let Some(error) = &draft.error {
        ui.text_wrapped(error);
    }
}
