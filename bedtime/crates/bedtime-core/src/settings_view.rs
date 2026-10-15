//! Presentation for the preferences registered by core. Persistence stays in settings.
use bedtime_api::settings::{SettingsChange, SettingsPageContext};
use bedtime_api::workbench::ShortcutRuntime;
use bedtime_ui::settings as rows;
use bevy::prelude::*;
use dear_imgui_rs::Ui;
use serde_json::{Value, json};

#[derive(Component, Default)]
struct SettingsEditors {
    keybindings: bedtime_workbench::keybindings::KeybindingEditor,
    new_viewer_extension: String,
}

pub(crate) fn register(world: &mut World) {
    world.init_resource::<bedtime_api::features::FeatureRegistry>();
    let mut registry = world.resource_mut::<bedtime_api::features::FeatureRegistry>();
    // install_settings is also called by headless fixtures and theme installation.
    if registry
        .settings()
        .iter()
        .any(|section| section.id == "bedtime.ui.settings")
    {
        return;
    }
    registry
        .register_with("bedtime.ui", |r| {
            r.settings_ui("bedtime.ui.settings", "Appearance", appearance)
        })
        .expect("UI settings install once");
    registry
        .register_with("bedtime.documents", |r| {
            r.settings_page("bedtime.documents.settings", "Documents", documents)
        })
        .expect("Document settings install once");
    registry
        .register_with("bedtime.workbench", |r| {
            r.settings_page(
                "bedtime.workbench.settings",
                "Panels and Keybindings",
                workbench,
            )
        })
        .expect("Workbench settings install once");
}

fn appearance(ui: &Ui, draft: &mut Value) -> bool {
    let mut changed = false;
    let mut font = draft["font"].as_str().expect("published font").to_owned();
    changed |= rows::row(ui, "Font family", || {
        let mut changed = false;
        if let Some(_combo) = ui.begin_combo("##value", &font) {
            for name in bedtime_ui::font::available_fonts() {
                if ui.selectable_config(&name).selected(name == font).build() {
                    font = name;
                    changed = true;
                }
            }
        }
        changed
    });
    if changed {
        draft["font"] = json!(font);
    }
    let mut size = draft["fontSize"].as_f64().expect("published font size");
    if rows::number(ui, "Font size", &mut size) {
        draft["fontSize"] = json!(size);
        changed = true;
    }
    let mut opacity = draft["background_opacity"]
        .as_f64()
        .expect("published opacity") as f32;
    if rows::slider(ui, "Background opacity", &mut opacity, 0.0, 1.0) {
        draft["background_opacity"] = json!(opacity);
        changed = true;
    }
    let mut animations = draft["ui_animations"]
        .as_bool()
        .expect("published animation setting");
    if rows::checkbox(ui, "Animate interface", &mut animations) {
        draft["ui_animations"] = json!(animations);
        changed = true;
    }
    changed
}

fn documents(
    world: &mut World,
    ui: &Ui,
    context: SettingsPageContext,
    changes: &mut Vec<SettingsChange>,
) {
    let mut autosave = context
        .snapshot
        .get("bedtime.documents", "autosave")
        .and_then(Value::as_bool)
        .expect("published autosave");
    if rows::checkbox(ui, "Autosave", &mut autosave) {
        changes.push(SettingsChange::set(
            "bedtime.documents",
            "autosave",
            autosave,
        ));
    }
    let mut delay = context
        .snapshot
        .get("bedtime.documents", "autosave_delay_ms")
        .and_then(Value::as_f64)
        .expect("published autosave delay");
    if rows::number(ui, "Autosave delay (ms)", &mut delay) {
        changes.push(SettingsChange::set(
            "bedtime.documents",
            "autosave_delay_ms",
            delay,
        ));
    }
    ui.spacing();
    ui.separator_with_text("Default file viewers");
    if world.get::<SettingsEditors>(context.panel).is_none() {
        world
            .entity_mut(context.panel)
            .insert(SettingsEditors::default());
    }
    let mut extension = std::mem::take(
        &mut world
            .get_mut::<SettingsEditors>(context.panel)
            .unwrap()
            .new_viewer_extension,
    );
    let original = context
        .snapshot
        .get("bedtime.documents", "default_viewers")
        .expect("published viewer associations");
    let mut draft = original.clone();
    default_viewers(ui, world, &mut draft, &mut extension);
    if &draft != original {
        changes.push(SettingsChange::set(
            "bedtime.documents",
            "default_viewers",
            draft,
        ));
    }
    world
        .get_mut::<SettingsEditors>(context.panel)
        .unwrap()
        .new_viewer_extension = extension;
}

fn workbench(
    world: &mut World,
    ui: &Ui,
    context: SettingsPageContext,
    changes: &mut Vec<SettingsChange>,
) {
    let mut add = context
        .snapshot
        .get("bedtime.workbench", "tab_bar_add_button")
        .and_then(Value::as_bool)
        .expect("published add button setting");
    if rows::checkbox(ui, "Show add panel button", &mut add) {
        changes.push(SettingsChange::set(
            "bedtime.workbench",
            "tab_bar_add_button",
            add,
        ));
    }
    ui.spacing();
    let mut icons = context
        .snapshot
        .get("bedtime.workbench", "tab_icons")
        .and_then(Value::as_bool)
        .expect("published tab icon setting");
    if rows::checkbox(ui, "Show tab icons", &mut icons) {
        changes.push(SettingsChange::set("bedtime.workbench", "tab_icons", icons));
    }
    ui.separator_with_text("Keybindings");
    if world.get::<SettingsEditors>(context.panel).is_none() {
        world
            .entity_mut(context.panel)
            .insert(SettingsEditors::default());
    }
    let mut editor = std::mem::take(
        &mut world
            .get_mut::<SettingsEditors>(context.panel)
            .unwrap()
            .keybindings,
    );
    if editor.is_recording()
        && world
            .get_non_send::<ShortcutRuntime>()
            .is_none_or(|runtime| runtime.recording != Some(context.panel))
    {
        editor.cancel_recording();
    }
    if let Some(registry) = world.get_resource::<bedtime_api::workbench::WorkbenchRegistry>() {
        let groups = registry
            .panels()
            .iter()
            .map(|panel| (panel.id.to_string(), panel.label.to_owned()))
            .collect();
        let mut overrides = context.snapshot.keybindings();
        let edit = editor.draw(ui, registry.shortcuts(), &groups, &mut overrides);
        if edit.suppress_shortcuts {
            context.shortcuts.suppress(ui);
        }
        if let Some(error) = edit.error {
            ui.text_wrapped(error);
        }
        if edit.changed {
            changes.push(SettingsChange::set(
                "bedtime.workbench",
                "keybindings",
                serde_json::to_value(overrides).expect("Keybindings serialize"),
            ));
        }
    }
    if let Some(mut runtime) = world.get_non_send_mut::<ShortcutRuntime>() {
        if editor.is_recording() {
            runtime.recording = Some(context.panel);
            context.shortcuts.suppress(ui);
        } else if runtime.recording == Some(context.panel) {
            runtime.recording = None;
        }
    }
    world
        .get_mut::<SettingsEditors>(context.panel)
        .unwrap()
        .keybindings = editor;
}

fn default_viewers(ui: &Ui, world: &World, value: &mut Value, new_viewer_extension: &mut String) {
    let viewers = world
        .resource::<bedtime_api::features::FeatureRegistry>()
        .viewers();
    let associations = value
        .as_object_mut()
        .expect("published viewer associations are an object");
    ui.text_wrapped("Choose which viewer opens each file extension. Unavailable viewers keep their saved associations.");
    let mut extensions = associations.keys().cloned().collect::<Vec<_>>();
    extensions.sort();
    for extension in extensions {
        let _id = ui.push_id(&extension);
        let selected = associations[&extension]
            .as_str()
            .expect("validated viewer ID")
            .to_owned();
        let label = viewers
            .iter()
            .find(|viewer| viewer.id.as_str() == selected)
            .map_or_else(
                || format!("Unavailable: {selected}"),
                |viewer| viewer.label.to_owned(),
            );
        rows::row(ui, &format!(".{extension}"), || {
            ui.set_next_item_width(
                (ui.content_region_avail()[0]
                    - ui.calc_text_size("Remove")[0]
                    - ui.current_font_size() * 2.0)
                    .max(1.0),
            );
            if let Some(_combo) = ui.begin_combo("##viewer", &label) {
                for viewer in viewers {
                    if (viewer.fallback || viewer.matches_path(&format!("file.{extension}")))
                        && ui
                            .selectable_config(viewer.label)
                            .selected(viewer.id.as_str() == selected)
                            .build()
                    {
                        associations.insert(extension.clone(), json!(viewer.id));
                    }
                }
            }
            ui.same_line();
            if ui.button("Remove") {
                associations.remove(&extension);
            }
            false
        });
    }
    rows::row(ui, "Extension", || {
        ui.input_text("##value", new_viewer_extension).build()
    });
    let extension = new_viewer_extension
        .trim()
        .trim_start_matches('.')
        .to_ascii_lowercase();
    let valid = !extension.is_empty()
        && extension
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'));
    rows::row(ui, "Add association", || {
        let _disabled = ui.begin_disabled_with_cond(!valid);
        if let Some(_combo) = ui.begin_combo("##value", "Choose viewer…") {
            for viewer in viewers {
                if valid
                    && (viewer.fallback || viewer.matches_path(&format!("file.{extension}")))
                    && ui.selectable(viewer.label)
                {
                    associations.insert(extension.clone(), json!(viewer.id));
                    new_viewer_extension.clear();
                }
            }
        }
        false
    });
}
