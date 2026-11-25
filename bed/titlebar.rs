//! The titlebar: the menu, where the platform has no native menu bar or
//! `WindowMenu` asks for it too (one button, or the bar, as `CompactMenu`
//! says), and the tool buttons that open (or drag out) panels.
use bedtime::file_explorer::FileExplorer;
use bedtime::ui::{BuiltinIcon, Icon, Tip, UiContext, UiFrame, egui};
use bedtime::workbench::{
    Action, ActionId, BedtimeWorldExt as _, InvocationContext, LayoutSurface, PanelFactory, Show,
    ShowPanel, StandardMenus, Titlebar, draw_menu_bar, draw_menu_button, native_menu_shown,
    panel_drag_source, settings, settings::Settings,
};
use bevy::prelude::*;

settings! {
    "Window" {
        /// The menus in the window's titlebar. They're always there where the
        /// platform has no menu bar of its own, or its bar isn't showing them.
        pub WindowMenu: bool {
            default: !cfg!(target_os = "macos"),
            key: "appearance.window_menu",
            label: "Menu in window",
            // Elsewhere there's no other menu to leave it to.
            visible: |_, _| cfg!(target_os = "macos"),
            apply: |world, _| fit(world),
        }
        /// The window's menus behind one button in the titlebar, rather than
        /// a menu bar across the window.
        pub CompactMenu: bool {
            default: true,
            key: "appearance.compact_menu",
            label: "Compact menu",
            visible: |world, _| world.resource::<Settings>().get::<WindowMenu>(),
            apply: |world, _| fit(world),
        }
    }
}

/// How the menus are drawn in the window, if they are.
#[derive(Clone, Copy, PartialEq)]
pub enum MenuStyle {
    Button,
    Bar,
}

/// As the settings say; always while the native menu bar isn't showing the
/// menus (not yet, or the platform has none).
pub fn menu_style(world: &World) -> Option<MenuStyle> {
    let settings = world.resource::<Settings>();
    let native = world
        .get_resource::<StandardMenus>()
        .is_some_and(|menus| native_menu_shown(world, menus.bar));
    (settings.get::<WindowMenu>() || !native).then(|| {
        if settings.get::<CompactMenu>() {
            MenuStyle::Button
        } else {
            MenuStyle::Bar
        }
    })
}

/// The titlebar's height: a row of tools (beside the window's buttons, on
/// macOS), under the menu bar where one is drawn.
pub fn height(menu: Option<MenuStyle>) -> f32 {
    if menu == Some(MenuStyle::Bar) {
        40.0
    } else if cfg!(target_os = "macos") {
        18.0
    } else {
        24.0
    }
}

/// Keep the titlebars as tall as their menu needs.
pub fn fit(world: &mut World) {
    let height = height(menu_style(world));
    let mut titlebars = world.query::<&mut Titlebar>();
    for mut titlebar in titlebars.iter_mut(world) {
        if titlebar.height != height {
            titlebar.height = height;
        }
    }
}

/// bEd's own icon (the app's `bed.svg`), on the menu button.
#[derive(Resource)]
pub struct MenuIcon(pub Handle<Icon>);

impl FromWorld for MenuIcon {
    fn from_world(world: &mut World) -> Self {
        let icon = Icon::from_svg(include_bytes!("../src/bed/assets/bed.svg"))
            .expect("bed.svg must be valid");
        Self(world.resource_mut::<Assets<Icon>>().add(icon))
    }
}

/// The menu, if it's drawn in the window, then the tools.
pub fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>) {
    let root = world.get_resource::<StandardMenus>().map(|menus| menus.bar);
    if let (Some(root), Some(style)) = (root, menu_style(world)) {
        let result = match style {
            MenuStyle::Button => {
                // At the left of the tools' row, which is drawn over it.
                let row = ui.available_rect_before_wrap();
                let mut left = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(row)
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                left.add_space(4.0);
                left.spacing_mut().button_padding = egui::vec2(5.0, 4.0);
                let icon = world.resource::<MenuIcon>().0.clone();
                draw_menu_button(&mut context.frame(&mut left), world, root, |frame| {
                    menu_button(frame, &icon)
                })
            }
            MenuStyle::Bar => {
                // Inset like the panels below it.
                let inset = world
                    .get::<LayoutSurface>(context.surface)
                    .map_or(0.0, |surface| surface.panel_gap);
                draw_menu_bar(&mut context.frame(ui), world, root, inset)
            }
        };
        world.bedtime_mut().notify_failure("Menu", result);
    }
    // Right to left: terminal, search, files, git, structure.
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.add_space(4.0);
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.spacing_mut().button_padding = egui::vec2(5.0, 4.0);
        tool::<bedtime::code::StructurePanel>(ui, world, context, BuiltinIcon::Structure);
        tool::<bedtime::git::GitPanel>(ui, world, context, BuiltinIcon::GitBranch);
        tool::<FileExplorer>(ui, world, context, BuiltinIcon::Folder);
        // Search is more than its panel: it's seeded with the selection.
        tool_for::<bedtime::search::SearchWorkspace>(
            ui,
            world,
            context,
            BuiltinIcon::Search,
            PanelFactory::of::<bedtime::search::SearchPanel>(),
        );
        tool::<bedtime::terminal::TerminalPanel>(ui, world, context, BuiltinIcon::Terminal);
    });
}

/// The button the menus open from: bEd's icon, named "Menu" for
/// accessibility and scripts.
fn menu_button(frame: &mut UiFrame, icon: &Handle<Icon>) -> egui::Response {
    let response = frame.icon_ink_button(icon, 16.0);
    let enabled = frame.ui.is_enabled();
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, "Menu"));
    response
}

/// A new panel from `factory` in the active layout's largest area,
/// focused (`Placement::Largest`).
fn open_in_largest(world: &mut World, factory: &PanelFactory) -> Result<(), String> {
    let layout = world.bedtime_mut().layouts_mut().ensure_active()?;
    let area = world.bedtime().layouts().destination_area(layout)?;
    let panel = (factory.create)(world, area);
    world.flush();
    if world.get_entity(panel).is_err() {
        return Err("It couldn't be placed".into());
    }
    world.bedtime_mut().layouts_mut().focus_panel(panel)
}

/// A titlebar button for panel `P`: its `Show<P>`'s label and keys, opening
/// one in the largest area on a click, or where it's dropped when dragged
/// out.
fn tool<P: ShowPanel>(
    ui: &mut egui::Ui,
    world: &mut World,
    context: &UiContext<'_>,
    icon: BuiltinIcon,
) {
    tool_for::<Show<P>>(ui, world, context, icon, PanelFactory::of::<P>());
}

/// A titlebar button for command `C`. Clicking it opens a new panel from
/// `factory` in the largest area; dragging it out, where it's dropped.
fn tool_for<C: Action>(
    ui: &mut egui::Ui,
    world: &mut World,
    context: &UiContext<'_>,
    icon: BuiltinIcon,
    factory: PanelFactory,
) {
    let command = ActionId::of::<C>();
    let enabled = world
        .bedtime()
        .actions()
        .available(&command, InvocationContext::Focused);
    let tooltip = match world
        .bedtime()
        .actions()
        .shortcut_for(&command, InvocationContext::Focused)
    {
        Some(chord) => format!("{} ({})", C::LABEL, chord.label()),
        None => C::LABEL.to_owned(),
    };
    ui.add_enabled_ui(enabled, |ui| {
        let response = context
            .frame(ui)
            .icon_ink_button(icon, 16.0)
            .interact(egui::Sense::click_and_drag());
        (&response).tip(tooltip);
        panel_drag_source(&response, factory.clone());
        if response.clicked() {
            // After this frame's drawing.
            world.commands().queue(move |world: &mut World| {
                let result = open_in_largest(world, &factory);
                world
                    .bedtime_mut()
                    .notify_failure("Couldn't open the panel", result);
            });
        }
    });
}
