//! Settings presentation borrows the world's concrete settings domain owner.
use super::{MODULE_ID, NEW_COMMAND, PANEL_ID, SettingsCategory, SettingsUi, SettingsWindowState};
use crate::config::Settings;
use bed_api::features::FeatureRegistry;
use bed_api::workbench::{
    CommandInvocation, CommandResult, Panel, PanelCreation, PanelResult, WorkbenchAction, register,
};
use bed_documents::document_runtime::DocumentRuntime;
use bed_ui::icons::Icons;
use bevy::prelude::*;
use dear_imgui_rs::{TreeNodeFlags, Ui};
use serde_json::{Value, json};

pub struct SettingsPlugin;

impl Plugin for SettingsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<bed_api::features::FeatureRegistry>();
        app.world_mut()
            .resource_mut::<bed_api::features::FeatureRegistry>()
            .register_with(MODULE_ID, |registrar| {
                super::panel::register_metadata(registrar);
            })
            .expect("feature metadata is installed once");
        register(app.world_mut(), MODULE_ID, |registrar| {
            registrar.panel(PANEL_ID, "Settings", create_panel);
            registrar.command(NEW_COMMAND, "New Settings", open_panel);
            registrar.command("bed.settings.show", "Show Settings", show_panel);
            registrar.global_shortcut("bed.settings.show", &["Primary+Comma"]);
            registrar.command_icon(NEW_COMMAND, "bed.core.settings");
            registrar.panel_icon(PANEL_ID, "bed.core.settings");
            registrar.toolbar(NEW_COMMAND);
        })
        .expect("SettingsPlugin must be installed once");
    }
}

/// Category selection belongs to the panel entity; text/theme drafts are local.
#[derive(Component, Default)]
pub struct SettingsPanelState {
    pub category: SettingsCategory,
    #[cfg(test)]
    section_points: Vec<[f32; 2]>,
}

pub struct SettingsFrontend {
    pub entity: Entity,
    scratch: DocumentRuntime,
    ui_state: SettingsWindowState,
}

fn open_panel(In(invocation): In<CommandInvocation>) -> CommandResult {
    Ok(vec![WorkbenchAction::OpenPanel {
        panel_type: PANEL_ID.into(),
        state: Value::Null,
        area: Some(invocation.area),
    }])
}

fn create_panel(In(creation): In<PanelCreation>, world: &mut World) -> PanelResult {
    world.entity_mut(creation.entity).insert(bed_api::workbench::PanelDocumentation::new(&[
        ("Preferences", "Choose a category to change appearance, editing, terminal, or plugin settings."),
        ("Keybindings", "Use the Keybindings category to assign or remove shortcuts. Docs reflects your current assignments."),
    ]));
    world
        .entity_mut(creation.entity)
        .insert(SettingsPanelState::default());
    Ok(Box::new(SettingsFrontend {
        entity: creation.entity,
        scratch: DocumentRuntime::new(),
        ui_state: SettingsWindowState::default(),
    }))
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
        _shortcuts: &bed_api::workbench::ShortcutInput,
    ) {
        if self.ui_state.shortcut_recording.is_some()
            && world
                .get_non_send::<bed_api::workbench::ShortcutRuntime>()
                .is_some_and(|runtime| runtime.recording != Some(self.entity))
        {
            self.ui_state.shortcut_recording = None;
        }
        self.ui_state.category = world
            .get::<SettingsPanelState>(self.entity)
            .expect("a live SettingsFrontend has SettingsPanelState")
            .category;
        let (sections, viewers) = world
            .get_resource::<FeatureRegistry>()
            .map(|registry| (registry.settings().to_vec(), registry.viewers().to_vec()))
            .unwrap_or_default();
        self.ui_state.viewers = viewers;
        self.ui_state.shortcut_groups = world
            .resource::<bed_api::workbench::WorkbenchRegistry>()
            .panels()
            .iter()
            .map(|p| (p.id.to_owned(), p.label.to_owned()))
            .collect();
        self.ui_state.shortcut_actions = world
            .resource::<bed_api::workbench::WorkbenchRegistry>()
            .shortcuts()
            .to_vec();
        if self.ui_state.shortcut_recording.is_some() {
            _shortcuts.suppress(ui);
        }
        let fonts = world.get_non_send::<bed_ui::font::UiFonts>();
        self.ui_state.main_font = fonts.and_then(|fonts| fonts.main);
        self.ui_state.font_warning = fonts.and_then(|fonts| fonts.warning.clone());
        // Only texture IDs are needed while drawing; no native/GPU owner escapes.
        let icons = world.get_non_send::<Icons>().map(|icons| Icons {
            images: Default::default(),
            textures: icons.textures.clone(),
        });
        #[cfg(test)]
        let mut section_points = Vec::new();
        {
            let Some(mut settings) = world.get_non_send_mut::<Settings>() else {
                ui.text_disabled("Settings are unavailable in this application");
                return;
            };
            settings.draw_tab_with_state(
                ui,
                &mut self.ui_state,
                &mut self.scratch,
                icons.as_ref(),
                &mut |ui, values| {
                    let mut changed = false;
                    for section in &sections {
                        let Some(draw) = section.draw else { continue };
                        let _id = ui.push_id(section.id);
                        let expanded = ui.collapsing_header(section.label, TreeNodeFlags::empty());
                        #[cfg(test)]
                        {
                            let (min, max) = ui.item_rect();
                            section_points.push([(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5]);
                        }
                        if expanded {
                            if !values["plugins"].is_object() {
                                values["plugins"] = json!({});
                            }
                            let namespace = &mut values["plugins"][section.plugin];
                            if !namespace.is_object() {
                                *namespace = json!({});
                            }
                            changed |= draw(ui, namespace);
                        }
                    }
                    changed
                },
            );
        }
        if self.ui_state.category != SettingsCategory::Keybindings {
            self.ui_state.shortcut_recording = None;
        }
        let overrides = world.non_send::<Settings>().keybinds.overrides.clone();
        if let Some(mut runtime) = world.get_non_send_mut::<bed_api::workbench::ShortcutRuntime>() {
            runtime.overrides = overrides;
        }
        if self.ui_state.shortcut_recording.is_some() {
            _shortcuts.suppress(ui);
            if let Some(mut runtime) =
                world.get_non_send_mut::<bed_api::workbench::ShortcutRuntime>()
            {
                runtime.recording = Some(self.entity);
            }
        } else if let Some(mut runtime) =
            world.get_non_send_mut::<bed_api::workbench::ShortcutRuntime>()
            && runtime.recording == Some(self.entity)
        {
            runtime.recording = None;
        }
        let mut state = world
            .get_mut::<SettingsPanelState>(self.entity)
            .expect("a live SettingsFrontend has SettingsPanelState");
        state.category = self.ui_state.category;
        #[cfg(test)]
        {
            state.section_points = section_points;
        }
    }
}

fn show_panel(In(invocation): In<CommandInvocation>) -> CommandResult {
    Ok(vec![WorkbenchAction::ShowPanel {
        panel_type: PANEL_ID.into(),
        state: Value::Null,
        area: Some(invocation.area),
    }])
}

#[cfg(test)]
mod tests {
    use super::*;
    use bed_workbench::{WorkbenchLayout, WorkbenchPlugin, WorkbenchRequests, capture_command};
    use dear_imgui_rs::{Condition, Context, FramePrepareOptions, MouseButton};
    use std::path::PathBuf;

    fn open(app: &mut App) -> Entity {
        let command = capture_command(app.world(), NEW_COMMAND, Value::Null);
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .push(WorkbenchAction::Invoke(command));
        app.update();
        assert!(
            app.world_mut()
                .resource_mut::<WorkbenchRequests>()
                .take_errors()
                .is_empty()
        );
        app.world().resource::<WorkbenchLayout>().focused.unwrap()
    }

    #[test]
    fn plugin_uses_live_typed_settings_and_app_registered_sections() {
        fn frame(context: &mut Context, app: &mut App, entity: Entity) {
            context.prepare_frame(FramePrepareOptions::new([1200.0, 1000.0], 1.0 / 60.0));
            let ui = context.frame();
            ui.window("Bevy settings fixture")
                .position([0.0; 2], Condition::Always)
                .size([1100.0, 900.0], Condition::Always)
                .build(|| {
                    assert!(
                        bed_workbench::render_panel(app.world_mut(), entity, ui)
                            .unwrap()
                            .is_empty()
                    );
                });
            drop(context.render_legacy());
            let requests = app.world_mut().non_send_mut::<Settings>().take_requests();
            for request in requests {
                bed_core::settings::apply_settings_request(app.world_mut(), &request).unwrap();
            }
            crate::config::sync_view(app.world_mut()).unwrap();
        }
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let directory = crate::settings_view::test_support::TempDir::new();
        let settings = Settings::with_paths(
            directory.path("config"),
            PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/")),
        )
        .unwrap();
        let path = settings.settings_path.clone();
        let mut app = App::new();
        app.add_plugins((WorkbenchPlugin, SettingsPlugin));
        app.insert_non_send(settings);
        app.init_resource::<FeatureRegistry>();
        bed_core::settings::initialize_settings(app.world_mut(), Some(path.clone())).unwrap();
        bed_api::settings::register(
            app.world_mut(),
            "fixture",
            vec![bed_api::settings::SettingDefinition::new(
                "enabled", "Enabled", false,
            )],
        )
        .unwrap();
        crate::config::register_app_settings(app.world_mut()).unwrap();
        bed_core::settings::finalize_settings(app.world_mut()).unwrap();
        crate::config::sync_view(app.world_mut()).unwrap();
        app.world_mut()
            .resource_mut::<FeatureRegistry>()
            .register_with("fixture", |registrar| {
                registrar.settings_ui("fixture.controls", "Fixture controls", |_, values| {
                    values["enabled"] = json!(true);
                    true
                });
            })
            .unwrap();
        let first = open(&mut app);
        let second = open(&mut app);
        app.world_mut()
            .get_mut::<SettingsPanelState>(first)
            .unwrap()
            .category = SettingsCategory::Extensions;
        assert_eq!(
            app.world()
                .get::<SettingsPanelState>(second)
                .unwrap()
                .category,
            SettingsCategory::General
        );
        let mut context = Context::create();
        context.set_ini_filename(None::<PathBuf>).unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        frame(&mut context, &mut app, first);
        let point = app
            .world()
            .get::<SettingsPanelState>(first)
            .unwrap()
            .section_points[0];
        context.io_mut().add_mouse_pos_event(point);
        frame(&mut context, &mut app, first);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        frame(&mut context, &mut app, first);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, false);
        frame(&mut context, &mut app, first);
        assert_eq!(
            app.world().non_send::<Settings>().settings["plugins"]["fixture"]["enabled"],
            true
        );
        assert_eq!(
            crate::config::read_json(&path).unwrap()["overrides"]["fixture"]["enabled"],
            true
        );
        assert_eq!(
            app.world()
                .get::<SettingsPanelState>(second)
                .unwrap()
                .category,
            SettingsCategory::General
        );
        let saved = bed_workbench::save(app.world_mut());
        bed_workbench::restore(app.world_mut(), &saved).unwrap();
        assert_eq!(bed_workbench::save(app.world_mut()), saved);
    }
}
