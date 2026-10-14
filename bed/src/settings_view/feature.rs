//! bEd owns theme files and menu policy; ordinary preferences belong to plugins.
use crate::themes::{ActiveTheme, ThemeDraft, ThemeFiles, ThemeResults};
use bedtime_api::settings::{SettingsChange, SettingsPageContext};
use bevy::prelude::*;
use dear_imgui_rs::Ui;
use std::collections::HashMap;

pub struct BedSettingsPagesPlugin;
impl Plugin for BedSettingsPagesPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<bedtime_plugin_settings::SettingsPlugin>() {
            app.add_plugins(bedtime_plugin_settings::SettingsPlugin);
        }
        app.init_non_send::<ThemePresentations>();
        app.world_mut()
            .resource_mut::<bedtime_api::features::FeatureRegistry>()
            .register_with("bed.settings", |registrar| {
                registrar.settings_page("bed.settings.themes", "Themes", draw_themes);
                registrar.settings_page("bed.settings.application", "bEd", draw_application);
            })
            .expect("bEd settings pages install once");
        app.add_systems(Last, retire_presentations);
    }
}
#[derive(Default)]
struct ThemePresentations(HashMap<Entity, ThemeDraft>);
fn retire_presentations(world: &mut World) {
    let alive = world
        .query_filtered::<Entity, With<bedtime_plugin_settings::SettingsPanelState>>()
        .iter(world)
        .collect::<std::collections::HashSet<_>>();
    world
        .non_send_mut::<ThemePresentations>()
        .0
        .retain(|entity, _| alive.contains(entity));
}
fn open_file(world: &mut World, panel: Entity, path: std::path::PathBuf) {
    bedtime_core::submit_host_request(
        world,
        Some(panel),
        bedtime_api::features::HostAction::OpenFile {
            path: path.to_string_lossy().into_owned(),
            viewer: None,
        },
    );
}
fn draw_application(
    world: &mut World,
    ui: &Ui,
    context: SettingsPageContext,
    _: &mut Vec<SettingsChange>,
) {
    ui.text_wrapped("New windows without an explicit path open the Startup layout. Window → Set Current Layout saves the current arrangement.");
    let local = !world.resource::<bedtime_files::FileServices>().is_remote();
    let _disabled = ui.begin_disabled_with_cond(!local);
    if ui.button("Open Settings JSON") {
        let path = world
            .resource::<bedtime_api::launch::ApplicationPaths>()
            .config_dir
            .join("settings.json");
        if !path.exists() {
            // The shared settings owner creates the profile before the ordered
            // file-open intention; this page never writes preferences itself.
            world.write_message(bedtime_api::settings::SettingsRequest::batch(Vec::new()));
        }
        open_file(world, context.panel, path);
    }
    if !local && ui.is_item_hovered_with_flags(dear_imgui_rs::ItemHoveredFlags::ALLOW_WHEN_DISABLED)
    {
        bedtime_ui::components::tooltip_text(
            ui,
            &bedtime_ui::UiFrameContext::for_ui(ui),
            "Settings files are stored on this computer. Open the JSON from a local session.",
        );
    }
}
fn draw_themes(
    world: &mut World,
    ui: &Ui,
    context: SettingsPageContext,
    changes: &mut Vec<SettingsChange>,
) {
    let Some(files) = world.get_resource::<ThemeFiles>().cloned() else {
        ui.text_disabled("Theme files are unavailable");
        return;
    };
    let active = world.resource::<ActiveTheme>().clone();
    let current = context
        .snapshot
        .get("bed.application", "theme")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(crate::themes::DEFAULT_THEME);
    let mut draft = world
        .non_send_mut::<ThemePresentations>()
        .0
        .remove(&context.panel)
        .unwrap_or_default();
    draft.synchronize(&mut world.resource_mut::<ThemeResults>());
    if let Some(error) = &active.error {
        ui.text_wrapped(error);
    }
    if let Some(_combo) = ui.begin_combo("Theme", &active.file.name) {
        for (id, name) in files.list_themes() {
            let _id = ui.push_id(&id);
            if ui.selectable_config(&name).selected(id == current).build() && id != current {
                match files.load_theme(&id) {
                    Ok(_) => changes.push(SettingsChange::set("bed.application", "theme", id)),
                    Err(error) => draft.error = Some(error.to_string()),
                }
            }
        }
    }
    if current.starts_with("themes/") {
        let local = !world.resource::<bedtime_files::FileServices>().is_remote();
        let _disabled = ui.begin_disabled_with_cond(!local);
        if ui.button("Open Theme JSON") {
            match files.theme_path(current) {
                Ok(path) => open_file(world, context.panel, path),
                Err(error) => draft.error = Some(error.to_string()),
            }
        }
    }
    ui.spacing();
    crate::themes::editor::draw(ui, &mut draft, &files, current);
    for request in draft.take_requests() {
        world.write_message(request);
    }
    world
        .non_send_mut::<ThemePresentations>()
        .0
        .insert(context.panel, draft);
}

#[cfg(all(test, feature = "editor"))]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use bedtime_api::settings::SettingsSnapshot;
    use serde_json::Value;

    #[test]
    fn open_settings_button_uses_the_shared_owner_and_opens_the_created_profile() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let temp = TempDir::new();
        let mut app = App::new();
        crate::application::runtime::install_test_infrastructure(&mut app, &temp.path("config"));
        app.add_plugins((
            bedtime_core::DocumentHostPlugin,
            bedtime_plugin_editor::EditorPlugin,
            BedSettingsPagesPlugin,
        ));
        crate::config::ensure_settings_view(app.world_mut()).unwrap();
        let panel = bedtime_core::open_panel(
            app.world_mut(),
            bedtime_plugin_settings::PANEL_ID,
            bedtime_api::documents::PanelInput::None,
            Value::Null,
            None,
            None,
            false,
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
                let snapshot = app.world().resource::<SettingsSnapshot>().clone();
                ui.window("bEd preferences")
                    .position([0.0, 0.0], dear_imgui_rs::Condition::Always)
                    .size([700.0, 400.0], dear_imgui_rs::Condition::Always)
                    .build(|| {
                        draw_application(
                            app.world_mut(),
                            ui,
                            SettingsPageContext {
                                panel,
                                section: "bed.settings.application",
                                shortcuts: Default::default(),
                                snapshot,
                            },
                            &mut Vec::new(),
                        );
                        let (min, max) = ui.item_rect();
                        point = [(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5];
                    });
                drop(imgui.render_legacy());
                point
            };
            draw(&mut imgui);
            let point = draw(&mut imgui);
            imgui.io_mut().add_mouse_pos_event(point);
            draw(&mut imgui);
            imgui
                .io_mut()
                .add_mouse_button_event(dear_imgui_rs::MouseButton::Left, true);
            draw(&mut imgui);
            imgui
                .io_mut()
                .add_mouse_button_event(dear_imgui_rs::MouseButton::Left, false);
            draw(&mut imgui);
        }
        let settings_requests = bevy::ecs::message::MessageCursor::default()
            .read(
                app.world()
                    .resource::<Messages<bedtime_api::settings::SettingsRequest>>(),
            )
            .count();
        assert_eq!(
            settings_requests, 1,
            "button must submit profile creation through the preference owner"
        );
        bedtime_core::resolve_documents(app.world_mut());
        let path = temp.path("config/settings.json");
        let value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(value["version"], 1);
        assert!(value["overrides"].is_object());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while bedtime_core::host_requests_pending(app.world()) {
            bedtime_core::poll_document_services(app.world_mut());
            bedtime_core::resolve_documents(app.world_mut());
            assert!(
                std::time::Instant::now() < deadline,
                "profile file opening did not complete"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let path = std::fs::canonicalize(path).unwrap();
        assert!(
            bedtime_core::active_panel_entities(app.world_mut())
                .into_iter()
                .any(|panel| {
                    app.world()
                        .get::<bedtime_api::documents::PanelBinding>(panel)
                        .is_some_and(|binding| {
                            binding.document.is_some_and(|document| {
                                app.world()
                                    .non_send::<bedtime_documents::DocumentSession>()
                                    .snapshot(document)
                                    .unwrap()
                                    .path
                                    == path.to_string_lossy()
                            })
                        })
                })
        );
    }
}
