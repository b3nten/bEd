//! The titlebar: the menu bar where the platform has no native one, and
//! the tool buttons that open (or drag out) panels.
use bedtime_code::ShowStructure;
use bedtime_file_explorer::{FileExplorer, ShowFiles};
use bedtime_git::ShowGit;
use bedtime_terminal::NewTerminal;
use bedtime_ui::{BuiltinIcon, UiContext, egui};
use bedtime_workbench::{
    Action, ActionId, ActionsWorldExt, InvocationContext, LayoutSurface, PanelFactory,
    StandardMenus, WorkbenchWorldExt as _, draw_menu_bar, panel_drag_source,
};
use bevy::prelude::*;

/// The menu bar, where the platform has no native one, then the tools.
pub fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>) {
    if let Some(root) = world.get_resource::<StandardMenus>().map(|menus| menus.bar) {
        // Inset like the panels below it.
        let inset = world
            .get::<LayoutSurface>(context.surface)
            .map_or(0.0, |surface| surface.panel_gap);
        let result = draw_menu_bar(&mut context.frame(ui), world, root, inset);
        world.workbench_mut().notify_failure("Menu", result);
    }
    // Right to left: terminal, search, files, git, structure, settings.
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.add_space(4.0);
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.spacing_mut().button_padding = egui::vec2(5.0, 4.0);
        tool::<ShowStructure>(
            ui,
            world,
            context,
            BuiltinIcon::Structure,
            PanelFactory::of::<bedtime_code::StructurePanel>(),
        );
        tool::<ShowGit>(
            ui,
            world,
            context,
            BuiltinIcon::GitBranch,
            PanelFactory::of::<bedtime_git::GitPanel>(),
        );
        tool::<ShowFiles>(
            ui,
            world,
            context,
            BuiltinIcon::Folder,
            PanelFactory::of::<FileExplorer>(),
        );
        tool::<bedtime_search::SearchWorkspace>(
            ui,
            world,
            context,
            BuiltinIcon::Search,
            PanelFactory::of::<bedtime_search::SearchPanel>(),
        );
        tool::<NewTerminal>(
            ui,
            world,
            context,
            BuiltinIcon::Terminal,
            PanelFactory::of::<bedtime_terminal::TerminalPanel>(),
        );
    });
}

/// A new panel from `factory` in the active layout's largest area,
/// focused (`Placement::Largest`).
fn open_in_largest(world: &mut World, factory: &PanelFactory) -> Result<(), String> {
    use bedtime_workbench::LayoutsWorldExt;
    let layout = world.layouts_mut().ensure_active()?;
    let area = world.layouts().destination_area(layout)?;
    let panel = (factory.create)(world, area);
    world.flush();
    if world.get_entity(panel).is_err() {
        return Err("It couldn't be placed".into());
    }
    world.layouts_mut().focus_panel(panel)
}

/// A titlebar button for command `C`. Clicking it opens a new panel from
/// `factory` in the largest area; dragging it out, where it's dropped.
fn tool<C: Action>(
    ui: &mut egui::Ui,
    world: &mut World,
    context: &UiContext<'_>,
    icon: BuiltinIcon,
    factory: PanelFactory,
) {
    let command = ActionId::of::<C>();
    let enabled = world
        .actions()
        .available(&command, InvocationContext::Focused);
    let tooltip = match world
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
        context.frame(ui).tooltip(&response, tooltip);
        panel_drag_source(&response, factory.clone());
        if response.clicked() {
            // After this frame's drawing.
            world.commands().queue(move |world: &mut World| {
                let result = open_in_largest(world, &factory);
                world
                    .workbench_mut()
                    .notify_failure("Couldn't open the panel", result);
            });
        }
    });
}
