use bedtime_ui2::{BuiltinIcon, UiContext, UiPlugin, egui};
use bedtime_workbench2::{
    ActiveLayout, ActiveWorkspace, Area, InActiveWorkspace, InLayout, InWorkspace, Layout,
    Titlebar, WindowBlur, WorkbenchCamera, WorkbenchPlugin, WorkbenchSurface, Workspace,
};
use bevy::{prelude::*, window::PrimaryWindow};

mod autosave;
mod close;
mod code;
mod cube;
mod effects;
mod file_editor;
mod files;
mod fonts;
mod git;
mod images;
mod models;
mod hex;
mod keybindings;
mod layouts;
mod lsp;
mod lsp_panel;
mod menus;
mod opening;
mod session;
mod settings;
mod status_bar;
mod storage;
mod tasks;
mod terminal;

fn main() -> AppExit {
    if let Some(exit) = bedtime_terminal2::handle_shell_command() {
        return exit;
    }

    let mut app = App::new();

    bedtime_model2::register_asset_source(&mut app);
    app.add_plugins((
        DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "bEd 2".into(),
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
        cube::CubePlugin,
        (bedtime_mascots2::MascotPlugin, bedtime_model2::ModelPlugin),
        code::CodePlugin,
        files::FilesPlugin,
        tasks::TasksPlugin,
        git::GitPlugin,
        settings::SettingsPlugin,
        layouts::LayoutsPlugin,
        autosave::AutosavePlugin,
        session::SessionPlugin,
        (close::ClosePlugin, bedtime_terminal2::TerminalPlugin),
    ))
    .add_systems(Startup, setup);
    bedtime_workbench2::register_document_handlers(app.world_mut())
        .expect("Application document handlers");

    menus::install(&mut app);
    settings::configure(&mut app);
    terminal::install(&mut app);
    files::configure_startup(&mut app);

    #[cfg(feature = "automation")]
    {
        use bedtime_automation2::AutomationAppExt;
        app.add_plugins(bedtime_automation2::AutomationPlugin::stdio())
            .init_resource::<ToolbarRects>();
        app.register_automation_command("app.inspect_models", |world, _| {
            let cameras: Vec<_> = world.query::<(Entity, &Camera, &ChildOf)>().iter(world).filter_map(|(entity,camera,parent)| {
                let owner = world.get::<ChildOf>(parent.parent())?.parent();
                world.get::<bedtime_model2::ModelScene>(owner)?;
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
            Ok(serde_json::json!({ "scenes": world.query::<(Entity, &bedtime_mascots2::MascotScene)>().iter(world)
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

    let workspace = commands
        .spawn((
            Workspace {
                name: "Default".into(),
                ..default()
            },
            ActiveWorkspace,
        ))
        .id();

    commands.queue(move |world: &mut World| {
        session::remember_initial_state(world, workspace)
            .expect("Valid application startup composition");
    });
}

fn draw_titlebar(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>) {
    close::draw_close_dialog(ui, world);
    // Native presentation suppresses this shared-definition fallback row.
    menus::draw_application_menu(ui, world, context);
    let layout = world
        .query_filtered::<Entity, (With<ActiveLayout>, With<InActiveWorkspace>)>()
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
        // Match bed1's native 26×22pt buttons, 2pt gaps and 10pt trailing inset.
        // The workbench titlebar already reserves 6pt at the window edge.
        ui.spacing_mut().button_padding = egui::vec2(5.0, 3.0);
        ui.add_space(4.0);
        for (icon, kind) in [
            (BuiltinIcon::Settings, layouts::ToolKind::Settings),
            (BuiltinIcon::Structure, layouts::ToolKind::Structure),
            (BuiltinIcon::Files, layouts::ToolKind::Files),
            (BuiltinIcon::GitBranch, layouts::ToolKind::Git),
            (BuiltinIcon::Terminal, layouts::ToolKind::Terminal),
        ] {
            let workspace = world
                .query_filtered::<Entity, With<ActiveWorkspace>>()
                .iter(world)
                .next();
            let ready = workspace.is_some_and(|w| storage::ready(world, w));
            let enabled = close::accepts_user_actions(world)
                && workspace.is_some()
                && (ready || kind == layouts::ToolKind::Settings);
            ui.add_enabled_ui(enabled, |ui| {
                ui.visuals_mut().button_frame = false;
                let mut frame = context.frame(ui);
                frame.theme = &toolbar_theme;
                let response = frame
                    .icon_button_sized(icon, 16.0)
                    .interact(egui::Sense::click_and_drag());
                #[cfg(feature = "automation")]
                tool_rects.push((kind.key(), response.rect));
                context.frame(ui).tooltip(
                    &response,
                    match kind {
                        layouts::ToolKind::Terminal => "Terminal",
                        layouts::ToolKind::Files => "Files",
                        layouts::ToolKind::Structure => "Structure",
                        layouts::ToolKind::Git => "Git",
                        _ => "Settings",
                    },
                );
                if ready {
                    bedtime_workbench2::panel_drag_source(
                        &response,
                        match kind {
                            layouts::ToolKind::Terminal => {
                                bedtime_workbench2::PanelFactory::new("Terminal", |w, a| {
                                    layouts::spawn_tool(w, a, layouts::ToolKind::Terminal)
                                })
                            }
                            layouts::ToolKind::Git => bedtime_workbench2::PanelFactory::of::<git::GitPanel>(),
                            layouts::ToolKind::Files => bedtime_workbench2::PanelFactory::of::<
                                bedtime_filetree2::FileTreePanel,
                            >(),
                            layouts::ToolKind::Structure => bedtime_workbench2::PanelFactory::of::<
                                bed_structure2::StructurePanel,
                            >(),
                            _ => bedtime_workbench2::PanelFactory::of::<settings::SettingsPanel>(),
                        },
                    );
                }
                if response.clicked() {
                    let workspace = workspace.unwrap();
                    let target = layout.unwrap_or_else(|| {
                        let layout = world
                            .spawn((
                                Layout {
                                    name: "Layout 1".into(),
                                    ..default()
                                },
                                InWorkspace(workspace),
                                ActiveLayout,
                                layouts::LayoutSlot::One,
                            ))
                            .id();
                        world.spawn((Area::default(), InLayout(layout)));
                        layout
                    });
                    if let Err(error) = layouts::open_or_focus(world, target, kind) {
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
