use bedtime_ui2::{BuiltinIcon, Theme, UiContext, UiPlugin, UsesTheme, egui, themes};
use bedtime_workbench2::{
    ActiveLayout, ActiveWorkspace, Area, InActiveWorkspace, InArea, InLayout, InWorkspace, Layout,
    Panel, PanelInstance, Titlebar, WindowBlur, Workbench, WorkbenchAppExt, WorkbenchCamera,
    WorkbenchPlugin, WorkbenchSurface, Workspace,
};
use bevy::{prelude::*, window::PrimaryWindow};

mod cube;
mod documents;
mod file_editor;
mod files;
mod tasks;
mod terminal;

fn main() -> AppExit {
    let mut app = App::new();

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
        cube::CubePlugin,
        tasks::TasksPlugin,
        documents::DocumentsPlugin,
        files::FilesPlugin,
        bedtime_terminal2::TerminalPlugin,
    ))
    .register_panel_type::<HelloWorld>()
    .register_panel_type::<ThemePanel>()
    .add_systems(Startup, setup);

    terminal::install(&mut app);
    files::configure_startup(&mut app);

    #[cfg(feature = "automation")]
    {
        app.add_plugins(bedtime_automation2::AutomationPlugin::stdio());
    }

    app.run()
}

fn setup(mut commands: Commands, window: Single<Entity, With<PrimaryWindow>>) {
    commands.entity(*window).insert((
        WorkbenchSurface::default(),
        Titlebar {
            height: 20.0,
            draw: draw_titlebar,
        },
        WindowBlur(false),
    ));

    commands.spawn((Camera2d, WorkbenchCamera));

    let workspace = commands
        .spawn((
            Workspace {
                name: "Main".into(),
                ..default()
            },
            ActiveWorkspace,
        ))
        .id();

    let layout = commands
        .spawn((
            Layout {
                name: "Default".into(),
                ..default()
            },
            InWorkspace(workspace),
            ActiveLayout,
        ))
        .id();

    let area = commands.spawn((Area::default(), InLayout(layout))).id();

    commands.spawn((PanelInstance::new(cube::CubeViewport::KEY), InArea(area)));
}

fn draw_titlebar(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>) {
    documents::draw_close_dialog(ui, world);
    let layout = world
        .query_filtered::<Entity, (With<ActiveLayout>, With<InActiveWorkspace>)>()
        .single(world)
        .ok();

    let response = ui
        .with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_enabled_ui(layout.is_some(), |ui| {
                let visuals = ui.visuals_mut();
                visuals.button_frame = false;
                visuals.override_text_color = None;
                let accent = bedtime_ui2::theme::color(context.theme.palette.accent);
                visuals.widgets.hovered.fg_stroke.color = accent;
                visuals.widgets.active.fg_stroke.color = accent;
                let settings = context.frame(ui).icon_button(BuiltinIcon::Settings);
                let documents = context.frame(ui).icon_button(BuiltinIcon::Folder);
                let files = context.frame(ui).icon_button(BuiltinIcon::Files);
                context.frame(ui).tooltip(&files, "Files");
                if files.clicked()
                    && let Some(layout) = layout
                {
                    files::open_files(world, layout);
                }
                context.frame(ui).tooltip(&documents, "Documents");
                if documents.clicked()
                    && let Some(layout) = layout
                {
                    documents::open_picker(world, layout);
                }
                settings
            })
            .inner
        })
        .inner;

    context.frame(ui).tooltip(&response, "Theme");

    if response.clicked()
        && let Some(layout) = layout
    {
        let mut workbench = Workbench::new(world);

        let existing = workbench.panels(layout).into_iter().find(|&panel| {
            workbench.world().get::<PanelInstance>(panel).unwrap().key == ThemePanel::KEY
        });

        if let Some(panel) = existing {
            workbench.focus_panel(panel).unwrap();
        } else {
            workbench.register_panel::<ThemePanel>(layout).unwrap();
        }
    }
}

struct HelloWorld;
impl Panel for HelloWorld {
    const KEY: &'static str = "bed.hello";
    const LABEL: &'static str = "Hello World";
    fn draw(
        ui: &mut bedtime_ui2::egui::Ui,
        _: &mut World,
        _: &bedtime_ui2::UiContext<'_>,
        _: Entity,
    ) {
        ui.label("Hello, world!");
    }
}

struct ThemePanel;
impl Panel for ThemePanel {
    const KEY: &'static str = "bed.theme";
    const LABEL: &'static str = "Theme";

    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, _: Entity) {
        let theme_entity = world.get::<UsesTheme>(context.surface).unwrap().0;
        let mut theme = world.get::<Theme>(theme_entity).unwrap().clone();
        let mut surface = *world.get::<WorkbenchSurface>(context.surface).unwrap();
        let mut blur = world.get::<WindowBlur>(context.surface).unwrap().0;

        context
            .frame(ui)
            .scroll("theme-editor", surface.rounding, |mut frame| {
                frame.padded(surface.padding, |frame| {
                    let ui = frame.ui;
                    ui.heading("Theme");
                    ui.weak("Changes apply live and are not saved.");
                    ui.horizontal(|ui| {
                        if ui.button("Dark preset").clicked() {
                            theme = themes::dark();
                        }
                        if ui.button("Light preset").clicked() {
                            theme = themes::light();
                        }
                    });

                    ui.separator();
                    ui.heading("Background");
                    ui.add(
                        egui::Slider::new(&mut theme.palette.background[3], 0.0..=1.0)
                            .text("Window opacity"),
                    );
                    ui.add(
                        egui::Slider::new(&mut theme.palette.panel_background[3], 0.0..=1.0)
                            .text("Panel opacity"),
                    );
                    ui.add_enabled(
                        cfg!(target_os = "macos"),
                        egui::Checkbox::new(&mut blur, "Background blur"),
                    )
                    .on_hover_text(
                        "Native window blur is available on macOS. Lower opacity to reveal it.",
                    );

                    ui.separator();
                    ui.heading("Panels");
                    ui.add(
                        egui::Slider::new(&mut surface.rounding, 0.0..=32.0)
                            .text("Border rounding"),
                    );
                    ui.add(egui::Slider::new(&mut surface.panel_gap, 0.0..=32.0).text("Gap"));
                    ui.checkbox(
                        &mut surface.single_area_edge_to_edge,
                        "Edge-to-edge for a single area",
                    )
                    .on_hover_text(
                        "Disables panel gaps and rounding while the layout has one area.",
                    );

                    ui.separator();
                    ui.heading("Colors");
                    egui::Grid::new("theme-colors")
                        .num_columns(2)
                        .show(ui, |ui| {
                            let p = &mut theme.palette;
                            for (label, value) in [
                                ("Window background", &mut p.background),
                                ("Panel background", &mut p.panel_background),
                                ("Surface", &mut p.surface),
                                ("Elevated", &mut p.elevated),
                                ("Border", &mut p.border),
                                ("Text", &mut p.text),
                                ("Muted", &mut p.muted),
                                ("Accent", &mut p.accent),
                                ("Selection", &mut p.selection),
                                ("Success", &mut p.success),
                                ("Warning", &mut p.warning),
                                ("Error", &mut p.error),
                            ] {
                                ui.label(label);
                                // Theme colors are unmultiplied sRGB; egui's float color
                                // editor expects linear RGB, so edit through sRGBA bytes.
                                let mut rgba = value.map(|v| (v * 255.0).round() as u8);
                                if ui.color_edit_button_srgba_unmultiplied(&mut rgba).changed() {
                                    *value = rgba.map(|v| v as f32 / 255.0);
                                }
                                ui.end_row();
                            }
                        });

                    ui.separator();
                    ui.heading("Terminal ANSI colors");
                    ui.weak("Default foreground and background colors are independent.");
                    egui::Grid::new("theme-terminal-colors")
                        .num_columns(3)
                        .show(ui, |ui| {
                            ui.label("");
                            ui.label("Normal");
                            ui.label("Bright");
                            ui.end_row();
                            for (index, label) in [
                                "Black", "Red", "Green", "Yellow", "Blue", "Magenta", "Cyan",
                                "White",
                            ]
                            .into_iter()
                            .enumerate()
                            {
                                ui.label(label);
                                for index in [index, index + 8] {
                                    let value = &mut theme.palette.terminal[index];
                                    let mut rgba = value.map(|v| (v * 255.0).round() as u8);
                                    if ui.color_edit_button_srgba_unmultiplied(&mut rgba).changed()
                                    {
                                        *value = rgba.map(|v| v as f32 / 255.0);
                                    }
                                }
                                ui.end_row();
                            }
                        });

                    ui.separator();
                    ui.heading("Dimensions");
                    egui::Grid::new("theme-dimensions")
                        .num_columns(2)
                        .show(ui, |ui| {
                            let m = &mut theme.metrics;
                            for (label, value, range) in [
                                ("Font size", &mut m.font_size, 8.0..=72.0),
                                ("Widget rounding", &mut m.rounding, 0.0..=64.0),
                                ("Popup rounding", &mut m.popup_rounding, 0.0..=64.0),
                                ("Popup gap", &mut m.popup_gap, 0.0..=128.0),
                            ] {
                                ui.label(label);
                                ui.add(egui::DragValue::new(value).speed(0.25).range(range));
                                ui.end_row();
                            }
                            for (label, value) in [
                                ("Widget padding", &mut m.padding),
                                ("Widget spacing", &mut m.spacing),
                                ("Tooltip padding", &mut m.tooltip_padding),
                                ("Menu padding", &mut m.menu_padding),
                                ("Menu spacing", &mut m.menu_spacing),
                                ("Panel content padding", &mut surface.padding),
                            ] {
                                ui.label(label);
                                ui.horizontal(|ui| {
                                    for (axis, value) in ["X ", "Y "].into_iter().zip(value) {
                                        ui.add(
                                            egui::DragValue::new(value)
                                                .prefix(axis)
                                                .speed(0.25)
                                                .range(0.0..=128.0),
                                        );
                                    }
                                });
                                ui.end_row();
                            }
                        });
                });
            });

        if world.get::<Theme>(theme_entity).unwrap() != &theme {
            world.entity_mut(theme_entity).insert(theme);
        }
        if world.get::<WorkbenchSurface>(context.surface) != Some(&surface) {
            world.entity_mut(context.surface).insert(surface);
        }
        if world.get::<WindowBlur>(context.surface).unwrap().0 != blur {
            world.entity_mut(context.surface).insert(WindowBlur(blur));
        }
    }
}
