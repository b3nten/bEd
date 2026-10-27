use bedtime_ui::{BuiltinIcon, UiContext, UiPlugin, egui};
use bedtime_workbench::{
    ActiveLayout, Area, InLayout, Layout, Panel, Titlebar, WindowBlur, WorkbenchCamera,
    WorkbenchPlugin, WorkbenchSurface,
};
use bevy::{prelude::*, window::PrimaryWindow};

mod autosave;
mod close;
mod code;
mod effects;
mod file_editor;
mod files;
mod fonts;
mod git;
mod images;
mod keybindings;
mod layouts;
mod lsp;
mod lsp_panel;
mod mascots;
mod menus;
mod models;
mod opening;
mod persistence;
mod project;
mod session;
mod settings;
mod status_bar;
mod storage;
mod terminal;

fn main() -> AppExit {
    if let Some(exit) = bedtime_terminal::handle_shell_command() {
        return exit;
    }

    let mut app = App::new();

    bedtime_model::register_asset_source(&mut app);
    app.add_plugins((
        DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "bEd".into(),
                resolution: (960, 640).into(),
                transparent: true,
                composite_alpha_mode: if cfg!(target_os = "macos") {
                    bevy::window::CompositeAlphaMode::PostMultiplied
                } else {
                    bevy::window::CompositeAlphaMode::Auto
                },
                titlebar_transparent: true,
                fullsize_content_view: true,
                titlebar_show_title: false,
                ..default()
            }),
            close_when_requested: false,
            ..default()
        }),
        UiPlugin,
        WorkbenchPlugin,
        effects::EffectsPlugin,
        (mascots::MascotsPlugin, bedtime_model::ModelPlugin),
        code::CodePlugin,
        files::FilesPlugin,
        git::GitPlugin,
        settings::SettingsPlugin,
        layouts::LayoutsPlugin,
        autosave::AutosavePlugin,
        session::SessionPlugin,
        (close::ClosePlugin, terminal::TerminalPlugin),
    ))
    .add_systems(Startup, setup);

    bedtime_workbench::register_document_handlers(app.world_mut())
        .expect("Application document handlers");

    menus::install(&mut app);
    settings::configure(&mut app);
    files::configure_startup(&mut app);

    #[cfg(feature = "automation")]
    app.init_resource::<ToolbarRects>();
    #[cfg(feature = "automation")]
    if !std::env::args_os().any(|argument| argument == "--no-automation") {
        use bedtime_automation::AutomationAppExt;
        app.add_plugins(bedtime_automation::AutomationPlugin::stdio())
            .init_resource::<ToolbarRects>();
        app.register_automation_command("app.inspect_models", |world, _| {
            let cameras: Vec<_> = world.query::<(Entity, &Camera, &ChildOf)>().iter(world).filter_map(|(entity,camera,parent)| {
                let owner = world.get::<ChildOf>(parent.parent())?.parent();
                world.get::<bedtime_model::ModelScene>(owner)?;
                Some(serde_json::json!({"panel":owner.to_bits().to_string(),"camera":entity.to_bits().to_string(),"active":camera.is_active}))
            }).collect();
            Ok(serde_json::json!({"cameras":cameras}))
        });
        app.register_automation_command("app.inspect_mascots", |world, _| {
            let cameras: Vec<_> = world.query::<(Entity, &Camera, &Transform, &ChildOf)>().iter(world)
                .filter_map(|(entity, camera, transform, parent)| {
                    let owner = world.get::<ChildOf>(parent.parent())?.parent();
                    Some((owner, entity, camera.is_active, transform.translation.to_array()))
                }).collect();
            Ok(serde_json::json!({ "scenes": world.query::<(Entity, &bedtime_mascots::MascotScene)>().iter(world)
                .map(|(entity, scene)| {
                    let camera = cameras.iter().find(|(owner, ..)| *owner == entity);
                    serde_json::json!({ "entity": entity.to_bits().to_string(),
                        "kind": format!("{:?}", scene.kind()), "error": scene.error(),
                        "active": camera.is_some_and(|c| c.2),
                        "camera": camera.map(|c| c.1.to_bits().to_string()),
                        "translation": camera.map(|c| c.3),
                    })
                }).collect::<Vec<_>>() }))
        });

        app.register_automation_command("app.inspect_titlebar", |world, _| {
            Ok(serde_json::json!({ "tools": world.resource::<ToolbarRects>().0.iter().map(|(key, rect)|
                serde_json::json!({ "key": key, "rect": { "min": [rect.min.x,rect.min.y], "max": [rect.max.x,rect.max.y] } })).collect::<Vec<_>>() }))
        });
    }

    app.run()
}

fn setup(mut commands: Commands, window: Single<Entity, With<PrimaryWindow>>) {
    commands.entity(*window).insert((
        WorkbenchSurface::default(),
        Titlebar {
            height: if cfg!(target_os = "macos") {
                18.0
            } else {
                40.0
            },
            draw: draw_titlebar,
        },
        WindowBlur(false),
    ));

    commands.spawn((Camera2d, WorkbenchCamera));

    commands.queue(|world: &mut World| {
        session::remember_initial_state(world).expect("Valid application startup composition");
    });
}

fn draw_titlebar(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>) {
    close::draw_close_dialog(ui, world);
    // Native presentation suppresses this shared-definition fallback row.
    menus::draw_application_menu(ui, world, context);
    let layout = world
        .query_filtered::<Entity, With<ActiveLayout>>()
        .single(world)
        .ok();

    #[cfg(feature = "automation")]
    let mut tool_rects = Vec::new();
    // Toolbar artwork keeps the theme's text ink through hover and press;
    // the shared icon button still animates its background normally.
    let mut toolbar_theme = context.theme.clone();
    toolbar_theme.palette.muted = toolbar_theme.palette.text;
    toolbar_theme.palette.accent = toolbar_theme.palette.text;
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        // Match bEd's native 26×22pt buttons, 2pt gaps and 10pt trailing inset.
        // The workbench titlebar already reserves 6pt at the window edge.
        ui.spacing_mut().button_padding = egui::vec2(5.0, 3.0);
        ui.add_space(4.0);
        for (icon, key) in [
            (BuiltinIcon::Settings, settings::SettingsPanel::KEY),
            (BuiltinIcon::Structure, bed_structure::StructurePanel::KEY),
            (BuiltinIcon::Files, bedtime_filetree::FileTreePanel::KEY),
            (BuiltinIcon::GitBranch, git::GitPanel::KEY),
            (BuiltinIcon::Terminal, bedtime_terminal::TerminalPanel::KEY),
        ] {
            let entry = menus::panel_menu(world, key).expect("Installed toolbar feature");
            let ready = storage::ready(world);
            let enabled = close::accepts_user_actions(world) && (entry.available)(world);
            ui.add_enabled_ui(enabled, |ui| {
                ui.visuals_mut().button_frame = false;
                let mut frame = context.frame(ui);
                frame.theme = &toolbar_theme;
                let response = frame
                    .icon_button_sized(icon, 16.0)
                    .interact(egui::Sense::click_and_drag());
                #[cfg(feature = "automation")]
                tool_rects.push((key, response.rect));
                context.frame(ui).tooltip(&response, entry.factory.label);
                if ready {
                    bedtime_workbench::panel_drag_source(&response, entry.factory.clone());
                }
                if response.clicked() {
                    let target = layout.unwrap_or_else(|| {
                        let layout = world
                            .spawn((
                                Layout {
                                    name: "Layout 1".into(),
                                    ..default()
                                },
                                ActiveLayout,
                                layouts::LayoutSlot::One,
                            ))
                            .id();
                        world.spawn((Area::default(), InLayout(layout)));
                        layout
                    });
                    if let Err(error) = menus::open_or_focus(world, target, key) {
                        bevy::log::warn!("Cannot open tool: {error}");
                    }
                }
            });
        }
    });

    #[cfg(feature = "automation")]
    {
        world.resource_mut::<ToolbarRects>().0 = tool_rects;
    }
}

#[cfg(feature = "automation")]
#[derive(Resource, Default)]
struct ToolbarRects(Vec<(&'static str, egui::Rect)>);

#[cfg(test)]
pub(crate) fn test_layout(world: &mut bevy::prelude::World) -> bevy::prelude::Entity {
    use bedtime_workbench::*;
    let layout = Workbench::new(world).create_layout("Test").unwrap();
    if active_layout(world).is_none() {
        world.entity_mut(layout).insert(ActiveLayout);
    }
    layout
}
