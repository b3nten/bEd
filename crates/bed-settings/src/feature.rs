//! Settings presentation borrows the world's concrete settings domain owner.
use super::{MODULE_ID, NEW_COMMAND, PANEL_ID, SettingsCategory, SettingsUi, SettingsWindowState};
use bed_config::Settings;
use bed_document_session::editor::Editor;
use bed_editor_api::Registry as EditorRegistry;
use bed_ui::icons::Icons;
use bed_workbench_api::{
    Action, CommandInvocation, CommandResult, Panel, PanelCreation, PanelResult, register,
};
use bevy::prelude::*;
use dear_imgui_rs::{TreeNodeFlags, Ui};
use serde_json::{Value, json};

pub struct SettingsPlugin;

impl Plugin for SettingsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<bed_editor_api::Registry>();
        app.world_mut()
            .resource_mut::<bed_editor_api::Registry>()
            .register_with(MODULE_ID, |registrar| {
                super::panel::register_metadata(registrar);
            })
            .expect("feature metadata is installed once");
        register(app.world_mut(), MODULE_ID, |registrar| {
            registrar.panel(PANEL_ID, "Settings", create_panel);
            registrar.command(NEW_COMMAND, "New Settings", open_panel);
            registrar.command("bed.settings.show", "Show Settings", show_panel);
            registrar.global_shortcut("bed.settings.show", &["Primary+Comma"]);
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
    scratch: Editor,
    ui_state: SettingsWindowState,
}

fn open_panel(In(invocation): In<CommandInvocation>) -> CommandResult {
    Ok(vec![Action::OpenPanel {
        panel_type: PANEL_ID.into(),
        state: Value::Null,
        area: Some(invocation.area),
    }])
}

fn create_panel(In(creation): In<PanelCreation>, world: &mut World) -> PanelResult {
    world.entity_mut(creation.entity).insert(bed_workbench_api::PanelDocumentation::new(&[
        ("Preferences", "Choose a category to change appearance, editing, terminal, or plugin settings."),
        ("Keybindings", "Use the Keybindings category to assign or remove shortcuts. Docs reflects your current assignments."),
    ]));
    world
        .entity_mut(creation.entity)
        .insert(SettingsPanelState::default());
    Ok(Box::new(SettingsFrontend {
        entity: creation.entity,
        scratch: Editor::new(),
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
        _: &mut Vec<Action>,
        _shortcuts: &bed_workbench_api::ShortcutInput,
    ) {
        if self.ui_state.shortcut_recording.is_some()
            && world
                .get_non_send::<bed_workbench_api::ShortcutRuntime>()
                .is_some_and(|runtime| runtime.recording != Some(self.entity))
        {
            self.ui_state.shortcut_recording = None;
        }
        self.ui_state.category = world
            .get::<SettingsPanelState>(self.entity)
            .expect("a live SettingsFrontend has SettingsPanelState")
            .category;
        let (sections, viewers) = world
            .get_resource::<EditorRegistry>()
            .map(|registry| (registry.settings().to_vec(), registry.viewers().to_vec()))
            .unwrap_or_default();
        self.ui_state.viewers = viewers;
        self.ui_state.shortcut_groups = world
            .resource::<bed_workbench_api::Registry>()
            .panels()
            .iter()
            .map(|p| (p.id.to_owned(), p.label.to_owned()))
            .collect();
        self.ui_state.shortcut_actions = world
            .resource::<bed_workbench_api::Registry>()
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
        if let Some(mut runtime) = world.get_non_send_mut::<bed_workbench_api::ShortcutRuntime>() {
            runtime.overrides = overrides;
        }
        if self.ui_state.shortcut_recording.is_some() {
            _shortcuts.suppress(ui);
            if let Some(mut runtime) =
                world.get_non_send_mut::<bed_workbench_api::ShortcutRuntime>()
            {
                runtime.recording = Some(self.entity);
            }
        } else if let Some(mut runtime) =
            world.get_non_send_mut::<bed_workbench_api::ShortcutRuntime>()
        {
            if runtime.recording == Some(self.entity) {
                runtime.recording = None;
            }
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
            .push(Action::Invoke(command));
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
        }
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let directory = crate::test_support::TempDir::new();
        let settings = Settings::with_paths(
            directory.path("config"),
            PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")),
        )
        .unwrap();
        let path = settings.settings_path.clone();
        let mut app = App::new();
        app.add_plugins((WorkbenchPlugin, SettingsPlugin));
        app.insert_non_send(settings);
        app.init_resource::<EditorRegistry>();
        app.world_mut()
            .resource_mut::<EditorRegistry>()
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
            bed_config::read_json(&path).unwrap()["plugins"]["fixture"]["enabled"],
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

fn show_panel(In(invocation): In<CommandInvocation>) -> CommandResult {
    Ok(vec![Action::ShowPanel {
        panel_type: PANEL_ID.into(),
        state: Value::Null,
        area: Some(invocation.area),
    }])
}
