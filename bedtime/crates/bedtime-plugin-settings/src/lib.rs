//! Optional preferences panel over core-owned registered settings.
pub mod keybindings;
use bedtime_api::settings::{
    SettingsChange, SettingsRegistry, SettingsRequest, SettingsRequestId, SettingsResults,
    SettingsSnapshot,
};
use bedtime_api::workbench::{
    CommandInvocation, CommandResult, Panel, PanelCreation, PanelDocumentation, PanelResult,
    ShortcutInput, ShortcutRuntime, WorkbenchAction,
};
use bevy::prelude::*;
use dear_imgui_rs::{TreeNodeFlags, Ui};
use serde_json::{Value, json};

pub const MODULE_ID: &str = "bedtime.settings";
pub const PANEL_ID: &str = "bedtime.settings.panel";
pub const NEW_COMMAND: &str = "bedtime.settings.new";

pub struct SettingsPlugin;
impl Plugin for SettingsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SettingsRegistry>()
            .init_resource::<bedtime_api::features::FeatureRegistry>()
            .add_message::<SettingsRequest>();
        app.world_mut()
            .resource_mut::<bedtime_api::features::FeatureRegistry>()
            .register_with(MODULE_ID, |registrar| {
                registrar.panel_options(
                    PANEL_ID,
                    false,
                    bedtime_api::workbench::PanelPlacement::Center,
                );
                registrar.command_ref(NEW_COMMAND);
            })
            .expect("Settings metadata installs once");
        bedtime_api::workbench::register(app.world_mut(), MODULE_ID, |registrar| {
            registrar.panel(PANEL_ID, "Settings", create_panel);
            registrar.command(NEW_COMMAND, "New Settings", open);
            registrar.command("bedtime.settings.show", "Show Settings", show);
            registrar.global_shortcut("bedtime.settings.show", &["Primary+Comma"]);
            registrar.command_icon(NEW_COMMAND, "bedtime.core.settings");
            registrar.panel_icon(PANEL_ID, "bedtime.core.settings");
            registrar.toolbar(NEW_COMMAND);
        })
        .expect("Settings panel installs once");
    }
}

#[derive(Component, Default)]
pub struct SettingsPanelState {
    pub page: Option<String>,
}
pub struct SettingsFrontend {
    entity: Entity,
    pending: Vec<SettingsRequestId>,
    error: Option<String>,
    keybinding_editor: keybindings::KeybindingEditor,
    new_viewer_extension: String,
}
fn create_panel(In(creation): In<PanelCreation>, world: &mut World) -> PanelResult {
    world.entity_mut(creation.entity).insert((
        SettingsPanelState::default(),
        PanelDocumentation::new(&[
            (
                "Preferences",
                "Registered preferences belong to the application. Reset restores its default.",
            ),
            (
                "Keybindings",
                "Assign shortcuts for application commands and focused panels.",
            ),
        ]),
    ));
    Ok(Box::new(SettingsFrontend {
        entity: creation.entity,
        pending: Vec::new(),
        error: None,
        keybinding_editor: Default::default(),
        new_viewer_extension: String::new(),
    }))
}
fn open(In(invocation): In<CommandInvocation>) -> CommandResult {
    Ok(vec![WorkbenchAction::OpenPanel {
        panel_type: PANEL_ID.into(),
        state: Value::Null,
        area: Some(invocation.area),
    }])
}
fn show(In(invocation): In<CommandInvocation>) -> CommandResult {
    Ok(vec![WorkbenchAction::ShowPanel {
        panel_type: PANEL_ID.into(),
        state: Value::Null,
        area: Some(invocation.area),
    }])
}

impl Panel for SettingsFrontend {
    fn title(&self, _: &World) -> String {
        "Settings".into()
    }
    fn draw(
        &mut self,
        ui: &Ui,
        world: &mut World,
        _: &mut Vec<WorkbenchAction>,
        shortcuts: &ShortcutInput,
    ) {
        let _style =
            bedtime_ui::components::controls_style(ui, &bedtime_ui::UiFrameContext::for_ui(ui));
        if self.keybinding_editor.is_recording()
            && world
                .get_non_send::<ShortcutRuntime>()
                .is_some_and(|runtime| runtime.recording != Some(self.entity))
        {
            self.keybinding_editor.cancel_recording();
        }
        if let Some(mut results) = world.get_resource_mut::<SettingsResults>() {
            self.pending.retain(|id| match results.take(*id) {
                Some(Ok(_)) => {
                    self.error = None;
                    false
                }
                Some(Err(error)) => {
                    self.error = Some(error);
                    false
                }
                None => true,
            });
        }
        let Some(snapshot) = world.get_resource::<SettingsSnapshot>().cloned() else {
            ui.text_disabled("Settings are unavailable in this application");
            return;
        };
        if let Some(error) = self.error.as_ref().or(snapshot.error.as_ref()) {
            ui.text_wrapped(error);
        }
        let registry = world.resource::<SettingsRegistry>().clone();
        let sections = world
            .get_resource::<bedtime_api::features::FeatureRegistry>()
            .map(|registry| registry.settings().to_vec())
            .unwrap_or_default();
        let mut changes = Vec::new();
        if let Some(layout) = world
            .get_resource::<bedtime_api::settings::SettingsPanelLayout>()
            .cloned()
        {
            let pages = layout
                .0
                .iter()
                .filter_map(|id| sections.iter().find(|section| section.id == id).cloned())
                .collect::<Vec<_>>();
            if pages.is_empty() {
                ui.text_disabled("No settings pages are installed");
                return;
            }
            let mut selected = world
                .get::<SettingsPanelState>(self.entity)
                .and_then(|state| state.page.clone())
                .filter(|id| pages.iter().any(|page| page.id == id))
                .unwrap_or_else(|| pages[0].id.into());
            if pages.len() > 1 {
                let width = (ui.current_font_size() * 9.0).min(ui.content_region_avail()[0] * 0.35);
                ui.child_window("SettingsCategories")
                    .size([width, 0.0])
                    .build(ui, || {
                        for page in &pages {
                            if ui
                                .selectable_config(page.label)
                                .selected(selected == page.id)
                                .build()
                            {
                                selected = page.id.into();
                            }
                        }
                    });
                ui.same_line();
            }
            world
                .get_mut::<SettingsPanelState>(self.entity)
                .expect("live settings panel")
                .page = Some(selected.clone());
            let page = pages
                .iter()
                .find(|page| page.id == selected)
                .expect("selected installed page");
            ui.child_window(format!("SettingsContent{}", page.id))
                .size([0.0, 0.0])
                .build(ui, || {
                    if let Some(draw) = page.draw_page {
                        draw(
                            world,
                            ui,
                            bedtime_api::settings::SettingsPageContext {
                                panel: self.entity,
                                section: page.id,
                                shortcuts: shortcuts.clone(),
                                snapshot: snapshot.clone(),
                            },
                            &mut changes,
                        );
                    } else if let Some(draw) = page.draw {
                        let original = snapshot.for_owner(page.plugin);
                        let mut draft = original.clone();
                        draw(ui, &mut draft);
                        if let Some(values) = draft.as_object() {
                            changes.extend(
                                values
                                    .iter()
                                    .filter(|(key, value)| original.get(*key) != Some(*value))
                                    .map(|(key, value)| {
                                        SettingsChange::set(page.plugin, key, value.clone())
                                    }),
                            );
                        }
                    }
                });
            self.submit_changes(world, changes);
            return;
        }
        let mut recording_visible = false;
        for (owner, definitions) in registry.owners() {
            let _id = ui.push_id(owner);
            let caption = sections
                .iter()
                .find(|section| section.plugin == owner)
                .map(|section| section.label)
                .unwrap_or(match owner {
                    "bedtime.ui" => "Appearance",
                    "bedtime.documents" => "Documents",
                    "bedtime.workbench" => "Panels and Keybindings",
                    "bedtime.editor" => "Code Editor",
                    _ => owner,
                });
            if !ui.collapsing_header(caption, TreeNodeFlags::empty()) {
                continue;
            }
            let original = snapshot.for_owner(owner);
            let mut draft = original.clone();
            if !draft.is_object() {
                draft = json!({});
            }
            for definition in definitions {
                let _id = ui.push_id(definition.key);
                let mut value = draft
                    .get(definition.key)
                    .cloned()
                    .unwrap_or_else(|| definition.default.clone());
                if owner == "bedtime.documents" && definition.key == "default_viewers" {
                    self.default_viewers(ui, world, &mut value);
                    if original.get(definition.key) != Some(&value) {
                        draft[definition.key] = value;
                    }
                    if ui.button("Reset viewer associations") {
                        changes.push(SettingsChange::reset(owner, definition.key));
                    }
                    continue;
                }
                if owner == "bedtime.ui" && definition.key == "font" {
                    let selected = value.as_str().expect("registered font is a string");
                    if let Some(_combo) = ui.begin_combo(definition.label, selected) {
                        for name in bedtime_ui::font::available_fonts() {
                            if ui
                                .selectable_config(&name)
                                .selected(name == selected)
                                .build()
                            {
                                draft[definition.key] = json!(name);
                            }
                        }
                    }
                    ui.same_line();
                    if ui.small_button("Reset") {
                        changes.push(SettingsChange::reset(owner, definition.key));
                    }
                    continue;
                }
                let changed = match &mut value {
                    Value::Bool(value) => ui.checkbox(definition.label, value),
                    Value::Number(value) => {
                        let mut number = value.as_f64().expect("JSON numbers are finite");
                        if ui.input_double(definition.label, &mut number) {
                            if let Some(number) = serde_json::Number::from_f64(number) {
                                *value = number;
                                true
                            } else {
                                false
                            }
                        } else {
                            false
                        }
                    }
                    Value::String(value) => ui.input_text(definition.label, value).build(),
                    _ => {
                        ui.text(definition.label);
                        false
                    }
                };
                if changed {
                    draft[definition.key] = value;
                }
                if owner == "bedtime.workbench" && definition.key == "keybindings" {
                    recording_visible = true;
                    self.keybindings(ui, world, shortcuts, &snapshot, &mut changes);
                    continue;
                }
                ui.same_line();
                if ui.small_button("Reset") {
                    changes.push(SettingsChange::reset(owner, definition.key));
                }
            }
            for section in sections.iter().filter(|section| section.plugin == owner) {
                if let Some(draw) = section.draw {
                    let _id = ui.push_id(section.id);
                    if ui.collapsing_header(section.label, TreeNodeFlags::empty()) {
                        draw(ui, &mut draft);
                    }
                }
            }
            if let Some(draft) = draft.as_object() {
                for (key, value) in draft {
                    if original.get(key) != Some(value)
                        && !changes.iter().any(|change| matches!(change, SettingsChange::Reset { owner: reset_owner, key: reset_key } if reset_owner == owner && reset_key == key)) {
                        changes.push(SettingsChange::set(owner, key, value.clone()));
                    }
                }
            }
        }
        for page in sections
            .iter()
            .filter(|section| section.draw_page.is_some())
        {
            if ui.collapsing_header(page.label, TreeNodeFlags::empty()) {
                page.draw_page.expect("custom page")(
                    world,
                    ui,
                    bedtime_api::settings::SettingsPageContext {
                        panel: self.entity,
                        section: page.id,
                        shortcuts: shortcuts.clone(),
                        snapshot: snapshot.clone(),
                    },
                    &mut changes,
                );
            }
        }
        self.submit_changes(world, changes);
        if !recording_visible {
            self.keybinding_editor.cancel_recording();
        }
        if let Some(mut runtime) = world.get_non_send_mut::<ShortcutRuntime>() {
            if self.keybinding_editor.is_recording() {
                runtime.recording = Some(self.entity);
                shortcuts.suppress(ui);
            } else if runtime.recording == Some(self.entity) {
                runtime.recording = None;
            }
        }
    }
}

impl SettingsFrontend {
    fn default_viewers(&mut self, ui: &Ui, world: &World, value: &mut Value) {
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
                .find(|viewer| viewer.id == selected)
                .map_or_else(
                    || format!("Unavailable: {selected}"),
                    |viewer| viewer.label.to_owned(),
                );
            ui.text(format!(".{extension}"));
            ui.same_line();
            if let Some(_combo) = ui.begin_combo("##viewer", &label) {
                for viewer in viewers {
                    if (viewer.fallback || viewer.matches_path(&format!("file.{extension}")))
                        && ui
                            .selectable_config(viewer.label)
                            .selected(viewer.id == selected)
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
        }
        ui.input_text("Extension", &mut self.new_viewer_extension)
            .build();
        let extension = self
            .new_viewer_extension
            .trim()
            .trim_start_matches('.')
            .to_ascii_lowercase();
        let valid = !extension.is_empty()
            && extension
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'));
        if let Some(_combo) = ui.begin_combo("Add association", "Choose viewer…") {
            for viewer in viewers {
                if valid
                    && (viewer.fallback || viewer.matches_path(&format!("file.{extension}")))
                    && ui.selectable(viewer.label)
                {
                    associations.insert(extension.clone(), json!(viewer.id));
                    self.new_viewer_extension.clear();
                }
            }
        }
    }
    fn submit_changes(&mut self, world: &mut World, changes: Vec<SettingsChange>) {
        if !changes.is_empty() {
            let request = SettingsRequest::batch(changes).tracked();
            self.pending.push(request.receipt.expect("Tracked request"));
            world.write_message(request);
        }
    }
    fn keybindings(
        &mut self,
        ui: &Ui,
        world: &World,
        shortcuts: &ShortcutInput,
        snapshot: &SettingsSnapshot,
        changes: &mut Vec<SettingsChange>,
    ) {
        let Some(registry) = world.get_resource::<bedtime_api::workbench::WorkbenchRegistry>()
        else {
            return;
        };
        let groups = registry
            .panels()
            .iter()
            .map(|panel| (panel.id.to_owned(), panel.label.to_owned()))
            .collect();
        let mut overrides = snapshot.keybindings();
        let edit = self
            .keybinding_editor
            .draw(ui, registry.shortcuts(), &groups, &mut overrides);
        if edit.suppress_shortcuts {
            shortcuts.suppress(ui);
        }
        if let Some(error) = edit.error {
            self.error = Some(error);
        }
        if edit.changed {
            changes.push(SettingsChange::set(
                "bedtime.workbench",
                "keybindings",
                serde_json::to_value(overrides).expect("Keybindings serialize"),
            ));
        }
    }
}

#[cfg(test)]
static IMGUI_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    use bedtime_api::settings::{SettingsPageContext, SettingsPanelLayout};
    use bevy::ecs::message::MessageCursor;

    fn custom_page(
        _: &mut World,
        ui: &Ui,
        context: SettingsPageContext,
        changes: &mut Vec<SettingsChange>,
    ) {
        ui.text("Application-owned presentation");
        assert!(
            !context
                .snapshot
                .get("example", "enabled")
                .unwrap()
                .as_bool()
                .unwrap()
        );
        changes.push(SettingsChange::set("example", "enabled", true));
    }

    #[test]
    fn custom_pages_keep_shared_panel_commands_and_submit_to_the_settings_owner() {
        let _lock = IMGUI_TEST_LOCK.lock().unwrap();
        let mut app = App::new();
        app.add_plugins((bedtime_workbench::WorkbenchPlugin, SettingsPlugin));
        bedtime_api::settings::register(
            app.world_mut(),
            "example",
            vec![bedtime_api::settings::SettingDefinition::new(
                "enabled", "Enabled", false,
            )],
        )
        .unwrap();
        app.world_mut()
            .resource_mut::<bedtime_api::features::FeatureRegistry>()
            .register_with("example", |r| {
                r.settings_page("example.custom", "Application", custom_page)
            })
            .unwrap();
        let snapshot = SettingsSnapshot {
            settings: json!({"example":{"enabled":false}}),
            ..Default::default()
        };
        app.insert_resource(snapshot)
            .insert_resource(SettingsPanelLayout(vec!["example.custom".into()]));
        let entity = bedtime_workbench::open_panel(
            app.world_mut(),
            json!({"panel_type":PANEL_ID,"state":null}),
            None,
            None,
        )
        .unwrap();
        let mut context = dear_imgui_rs::Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        context.io_mut().set_display_size([800.0, 600.0]);
        context.io_mut().set_delta_time(1.0 / 60.0);
        let ui = context.frame();
        ui.window("Preferences")
            .size([600.0, 400.0], dear_imgui_rs::Condition::Always)
            .build(|| {
                bedtime_workbench::render_panel(app.world_mut(), entity, ui).unwrap();
            });
        drop(context.render_legacy());
        assert_eq!(
            app.world()
                .get::<SettingsPanelState>(entity)
                .unwrap()
                .page
                .as_deref(),
            Some("example.custom")
        );
        assert_eq!(
            app.world()
                .resource::<SettingsSnapshot>()
                .get("example", "enabled"),
            Some(&Value::Bool(false))
        );
        let requests = MessageCursor::default()
            .read(app.world().resource::<Messages<SettingsRequest>>())
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].receipt.is_some());
        assert!(
            app.world()
                .resource::<bedtime_api::workbench::WorkbenchRegistry>()
                .command("bedtime.settings.show")
                .is_some()
        );
    }
}

#[cfg(test)]
mod viewer_tests {
    use super::*;
    use bevy::ecs::message::MessageCursor;
    #[test]
    fn reset_associations_requests_the_registered_default_and_preserves_unknown_viewers_until_reset()
     {
        let _lock = IMGUI_TEST_LOCK.lock().unwrap();
        let mut app = App::new();
        app.add_plugins((bedtime_workbench::WorkbenchPlugin, SettingsPlugin));
        bedtime_api::settings::register(
            app.world_mut(),
            "bedtime.documents",
            vec![bedtime_api::settings::SettingDefinition::new(
                "default_viewers",
                "Default viewers",
                json!({"sql":"application.sql"}),
            )],
        )
        .unwrap();
        let original = json!({"abc":"uninstalled.viewer"});
        app.insert_resource(SettingsSnapshot {
            settings: json!({"bedtime.documents":{"default_viewers":original}}),
            ..Default::default()
        });
        let panel = bedtime_workbench::open_panel(
            app.world_mut(),
            json!({"panel_type":PANEL_ID,"state":null}),
            None,
            None,
        )
        .unwrap();
        let mut imgui = dear_imgui_rs::Context::create();
        imgui.set_ini_filename(None::<std::path::PathBuf>).unwrap();
        imgui
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        {
            let mut draw = |imgui: &mut dear_imgui_rs::Context| {
                imgui.prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
                    [800.0, 600.0],
                    1.0 / 60.0,
                ));
                let ui = imgui.frame();
                let mut point = [0.0; 2];
                ui.window("Preferences")
                    .position([0.0, 0.0], dear_imgui_rs::Condition::Always)
                    .size([700.0, 500.0], dear_imgui_rs::Condition::Always)
                    .build(|| {
                        ui.set_next_item_open(true);
                        bedtime_workbench::render_panel(app.world_mut(), panel, ui).unwrap();
                        let (min, max) = ui.item_rect();
                        point = [(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5];
                    });
                drop(imgui.render_legacy());
                point
            };
            let point = draw(&mut imgui);
            imgui.io_mut().add_mouse_pos_event(point);
            imgui
                .io_mut()
                .add_mouse_button_event(dear_imgui_rs::MouseButton::Left, true);
            draw(&mut imgui);
            imgui
                .io_mut()
                .add_mouse_button_event(dear_imgui_rs::MouseButton::Left, false);
            draw(&mut imgui);
        }
        let requests = MessageCursor::default()
            .read(app.world().resource::<Messages<SettingsRequest>>())
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].changes,
            vec![SettingsChange::reset(
                "bedtime.documents",
                "default_viewers"
            )]
        );
        assert_eq!(
            app.world()
                .resource::<SettingsSnapshot>()
                .get("bedtime.documents", "default_viewers"),
            Some(&original)
        );
        assert_eq!(
            app.world()
                .resource::<SettingsRegistry>()
                .definition("bedtime.documents", "default_viewers")
                .unwrap()
                .default,
            json!({"sql":"application.sql"})
        );
    }
}
