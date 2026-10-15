//! Optional preferences panel over core-owned registered settings.
use bedtime_api::settings::{
    SettingsChange, SettingsRegistry, SettingsRequest, SettingsRequestId, SettingsResults,
    SettingsSnapshot,
};
use bedtime_api::workbench::{
    CommandInvocation, CommandResult, Panel, PanelCreation, PanelDocumentation, PanelResult,
    ShortcutInput, ShortcutRuntime, WorkbenchAction,
};
use bevy::prelude::*;
use dear_imgui_rs::Ui;
use serde_json::Value;

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
            registrar.command_icon(NEW_COMMAND, "settings");
            registrar.panel_icon(PANEL_ID, "settings");
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
    fade: bedtime_ui::util::transition::FadeIn,
}
fn create_panel(In(creation): In<PanelCreation>, world: &mut World) -> PanelResult {
    world.entity_mut(creation.entity).insert((
        SettingsPanelState::default(),
        PanelDocumentation::new(&[
            (
                "Preferences",
                "Changes apply immediately and are saved by the application.",
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
        fade: Default::default(),
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
        let mut pages = world
            .get_resource::<bedtime_api::features::FeatureRegistry>()
            .map(|registry| {
                registry
                    .settings()
                    .iter()
                    .filter(|section| section.draw.is_some() || section.draw_page.is_some())
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if let Some(layout) = world.get_resource::<bedtime_api::settings::SettingsPanelLayout>() {
            pages.retain(|page| layout.0.iter().any(|id| id == page.id));
        }
        pages.sort_by_key(|page| (page.label.to_lowercase(), page.id));
        let mut changes = Vec::new();
        if let Some(header) = world
            .get_resource::<bedtime_api::settings::SettingsPanelHeader>()
            .copied()
        {
            (header.0)(world, ui, self.entity);
            ui.separator();
        }
        if pages.is_empty() {
            ui.text_disabled("No settings sections are installed");
            self.submit_changes(world, changes);
            return;
        }
        let previous = world
            .get::<SettingsPanelState>(self.entity)
            .and_then(|state| state.page.clone());
        let mut selected = previous
            .clone()
            .filter(|id| pages.iter().any(|page| page.id == id))
            .unwrap_or_else(|| pages[0].id.into());
        let desired_width = pages
            .iter()
            .map(|page| ui.calc_text_size(page.label)[0])
            .fold(0.0, f32::max)
            + ui.current_font_size() * 1.5;
        let width = desired_width.min(ui.content_region_avail()[0] * 0.35);
        ui.child_window("SettingsCategories")
            .size([width, 0.0])
            .build(ui, || {
                for page in &pages {
                    let _id = ui.push_id(page.id);
                    if ui
                        .selectable_config(page.label)
                        .selected(selected == page.id)
                        .build()
                    {
                        selected = page.id.into();
                    }
                    if ui.is_item_hovered()
                        && ui.calc_text_size(page.label)[0] > width - ui.current_font_size()
                    {
                        bedtime_ui::components::tooltip_text(
                            ui,
                            &bedtime_ui::UiFrameContext::for_ui(ui),
                            page.label,
                        );
                    }
                }
            });
        ui.same_line();
        if previous.as_deref() != Some(&selected)
            && let Some(mut runtime) = world.get_non_send_mut::<ShortcutRuntime>()
            && runtime.recording == Some(self.entity)
        {
            runtime.recording = None;
        }
        world
            .get_mut::<SettingsPanelState>(self.entity)
            .expect("live settings panel")
            .page = Some(selected.clone());
        let page = pages
            .iter()
            .find(|page| page.id == selected)
            .expect("selected installed section");
        // One stable child lets the fade restart on selection rather than window creation.
        ui.child_window("SettingsContent")
            .size([0.0, 0.0])
            .build(ui, || {
                let _id = ui.push_id(page.id);
                if previous.as_deref() != Some(&selected) {
                    ui.set_scroll_y(0.0);
                }
                self.fade.draw(ui, page.id, || {
                    ui.text(page.label);
                    ui.separator();
                    ui.spacing();
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
            });
        self.submit_changes(world, changes);
    }
}

impl SettingsFrontend {
    fn submit_changes(&mut self, world: &mut World, changes: Vec<SettingsChange>) {
        if !changes.is_empty() {
            let request = SettingsRequest::batch(changes).tracked();
            self.pending.push(request.receipt.expect("Tracked request"));
            world.write_message(request);
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
    use serde_json::json;

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
                .command(&bedtime_api::CommandId::new("bedtime.settings.show"))
                .is_some()
        );
    }
    #[derive(Resource, Default)]
    struct DrawnSections(Vec<&'static str>);
    fn record_page(
        world: &mut World,
        ui: &Ui,
        context: SettingsPageContext,
        _: &mut Vec<SettingsChange>,
    ) {
        world
            .resource_mut::<DrawnSections>()
            .0
            .push(context.section);
        ui.text("Provider controls");
    }

    #[test]
    fn sidebar_sorts_display_names_and_draws_only_the_selected_provider() {
        let _lock = IMGUI_TEST_LOCK.lock().unwrap();
        let mut app = App::new();
        app.add_plugins((bedtime_workbench::WorkbenchPlugin, SettingsPlugin));
        app.insert_resource(SettingsSnapshot::default())
            .init_resource::<DrawnSections>();
        app.world_mut()
            .resource_mut::<bedtime_api::features::FeatureRegistry>()
            .register_with("example", |r| {
                r.settings_page("example.zulu", "Zulu", record_page);
                r.settings_page("example.appearance", "Appearance", record_page);
            })
            .unwrap();
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
        let draw = |context: &mut dear_imgui_rs::Context, app: &mut App| {
            context.prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
                [800.0, 600.0],
                1.0 / 60.0,
            ));
            let ui = context.frame();
            let mut second = [0.0; 2];
            ui.window("Preferences")
                .position([0.0, 0.0], dear_imgui_rs::Condition::Always)
                .size([700.0, 500.0], dear_imgui_rs::Condition::Always)
                .build(|| {
                    let origin = ui.cursor_screen_pos();
                    let padding = ui.clone_style().window_padding();
                    let height = ui.current_font_size() + ui.clone_style().item_spacing()[1];
                    second = [
                        origin[0] + padding[0] + 30.0,
                        origin[1] + padding[1] + height * 1.5,
                    ];
                    bedtime_workbench::render_panel(app.world_mut(), entity, ui).unwrap();
                });
            drop(context.render_legacy());
            second
        };
        let second = draw(&mut context, &mut app);
        assert_eq!(
            app.world().resource::<DrawnSections>().0,
            ["example.appearance"]
        );
        context.io_mut().add_mouse_pos_event(second);
        draw(&mut context, &mut app);
        context
            .io_mut()
            .add_mouse_button_event(dear_imgui_rs::MouseButton::Left, true);
        draw(&mut context, &mut app);
        context
            .io_mut()
            .add_mouse_button_event(dear_imgui_rs::MouseButton::Left, false);
        draw(&mut context, &mut app);
        assert_eq!(
            app.world()
                .get::<SettingsPanelState>(entity)
                .unwrap()
                .page
                .as_deref(),
            Some("example.zulu")
        );
        assert_eq!(
            app.world().resource::<DrawnSections>().0.last(),
            Some(&"example.zulu")
        );
        assert_eq!(
            app.world().resource::<DrawnSections>().0.len(),
            4,
            "one provider runs per frame"
        );
        assert!(
            app.world()
                .resource::<Messages<SettingsRequest>>()
                .is_empty()
        );
    }
}
