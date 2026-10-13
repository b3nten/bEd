//! Optional preferences panel over core-owned registered settings.
use bed_api::settings::{
    SettingsChange, SettingsRegistry, SettingsRequest, SettingsRequestId, SettingsResults,
    SettingsSnapshot,
};
use bed_api::workbench::{
    CommandInvocation, CommandResult, Panel, PanelCreation, PanelDocumentation, PanelResult,
    ShortcutInput, ShortcutRuntime, WorkbenchAction,
};
use bevy::prelude::*;
use dear_imgui_rs::{TreeNodeFlags, Ui};
use serde_json::{Value, json};

pub const MODULE_ID: &str = "bed.settings";
pub const PANEL_ID: &str = "bed.settings.panel";
pub const NEW_COMMAND: &str = "bed.settings.new";

pub struct SettingsPlugin;
impl Plugin for SettingsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SettingsRegistry>()
            .init_resource::<bed_api::features::FeatureRegistry>()
            .add_message::<SettingsRequest>();
        app.world_mut()
            .resource_mut::<bed_api::features::FeatureRegistry>()
            .register_with(MODULE_ID, |registrar| {
                registrar.panel_options(
                    PANEL_ID,
                    false,
                    bed_api::workbench::PanelPlacement::Center,
                );
                registrar.command_ref(NEW_COMMAND);
            })
            .expect("Settings metadata installs once");
        bed_api::workbench::register(app.world_mut(), MODULE_ID, |registrar| {
            registrar.panel(PANEL_ID, "Settings", create_panel);
            registrar.command(NEW_COMMAND, "New Settings", open);
            registrar.command("bed.settings.show", "Show Settings", show);
            registrar.global_shortcut("bed.settings.show", &["Primary+Comma"]);
            registrar.command_icon(NEW_COMMAND, "bed.core.settings");
            registrar.panel_icon(PANEL_ID, "bed.core.settings");
            registrar.toolbar(NEW_COMMAND);
        })
        .expect("Settings panel installs once");
    }
}

#[derive(Component, Default)]
pub struct SettingsPanelState;
pub struct SettingsFrontend {
    entity: Entity,
    pending: Vec<SettingsRequestId>,
    error: Option<String>,
    filter: String,
    recording: Option<(String, Option<usize>)>,
}
fn create_panel(In(creation): In<PanelCreation>, world: &mut World) -> PanelResult {
    world.entity_mut(creation.entity).insert((
        SettingsPanelState,
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
        filter: String::new(),
        recording: None,
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
            .get_resource::<bed_api::features::FeatureRegistry>()
            .map(|registry| registry.settings().to_vec())
            .unwrap_or_default();
        let mut changes = Vec::new();
        let mut recording_visible = false;
        for (owner, definitions) in registry.owners() {
            let _id = ui.push_id(owner);
            let caption = sections
                .iter()
                .find(|section| section.plugin == owner)
                .map(|section| section.label)
                .unwrap_or(match owner {
                    "bed.ui" => "Appearance",
                    "bed.documents" => "Documents",
                    "bed.workbench" => "Panels and Keybindings",
                    "bed.editor" => "Code Editor",
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
                if owner == "bed.workbench" && definition.key == "keybindings" {
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
        if !changes.is_empty() {
            let request = SettingsRequest::batch(changes).tracked();
            self.pending.push(request.receipt.expect("Tracked request"));
            world.write_message(request);
        }
        if !recording_visible {
            self.recording = None;
        }
        if let Some(mut runtime) = world.get_non_send_mut::<ShortcutRuntime>() {
            if self.recording.is_some() {
                runtime.recording = Some(self.entity);
                shortcuts.suppress(ui);
            } else if runtime.recording == Some(self.entity) {
                runtime.recording = None;
            }
        }
    }
}

impl SettingsFrontend {
    fn keybindings(
        &mut self,
        ui: &Ui,
        world: &World,
        shortcuts: &ShortcutInput,
        snapshot: &SettingsSnapshot,
        changes: &mut Vec<SettingsChange>,
    ) {
        let Some(registry) = world.get_resource::<bed_api::workbench::WorkbenchRegistry>() else {
            return;
        };
        let mut overrides = snapshot.keybindings();
        let bindings = bed_keybindings::Bindings::new(
            registry.shortcuts().to_vec(),
            &overrides,
            bed_keybindings::Platform::current(),
        );
        ui.input_text("Filter shortcuts", &mut self.filter).build();
        let filter = self.filter.to_lowercase();
        let mut changed = false;
        if let Some((action, index)) = self.recording.clone() {
            shortcuts.suppress(ui);
            ui.text("Press a shortcut");
            if let Some(stroke) = bed_keybindings::record(ui) {
                let action = registry
                    .shortcuts()
                    .iter()
                    .find(|candidate| candidate.id == action)
                    .expect("Recording a registered action");
                match overrides.replace(action, index, stroke) {
                    Ok(()) => changed = true,
                    Err(error) => self.error = Some(error),
                }
                self.recording = None;
            }
            if ui.small_button("Cancel recording") {
                self.recording = None;
            }
        }
        for action in registry.shortcuts().iter().filter(|action| {
            action.label.to_lowercase().contains(&filter)
                || action.id.to_lowercase().contains(&filter)
        }) {
            let _id = ui.push_id(&action.id);
            ui.text(&action.label);
            for (index, binding) in bindings.for_action(&action.id).iter().enumerate() {
                let _id = ui.push_id(&index.to_string());
                if ui.small_button(binding.shortcut.to_string()) {
                    self.recording = Some((action.id.clone(), Some(index)));
                }
                ui.same_line();
                if ui.small_button("Remove") {
                    match overrides.remove(action, index) {
                        Ok(()) => changed = true,
                        Err(error) => self.error = Some(error),
                    }
                }
            }
            if ui.small_button("Add shortcut") {
                self.recording = Some((action.id.clone(), None));
            }
            ui.same_line();
            if ui.small_button("Defaults") {
                overrides.actions.remove(&action.id);
                changed = true;
            }
        }
        for collision in bindings
            .collisions()
            .iter()
            .filter(|collision| collision.kind != bed_keybindings::CollisionKind::Reuse)
        {
            ui.text_wrapped(format!(
                "{}: {} overlaps {}",
                collision.shortcut, collision.first, collision.second
            ));
        }
        if changed {
            changes.push(SettingsChange::set(
                "bed.workbench",
                "keybindings",
                serde_json::to_value(overrides).expect("Keybindings serialize"),
            ));
        }
    }
}
