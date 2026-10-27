//! Optional UI presentation and input observations. UI has no workbench dependency.
use crate::*;
use bedtime_ui::{UiFrame, UiPlugin, UiSystems};
use bevy::prelude::*;

/// Screen observations of the egui presentation. Native menus remain independent.
#[derive(Component, Default, Clone, Copy)]
pub struct MenuPresentation {
    pub visible: bool,
    pub rect: Rect,
}

pub struct WorkbenchUiPlugin;
impl Plugin for WorkbenchUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PreUpdate,
            crate::surface::configure.before(UiSystems::Configure),
        );
        #[cfg(feature = "native")]
        app.configure_sets(
            PreUpdate,
            NativeSystems::WindowGeometry.before(UiSystems::Draw),
        );
    }
    fn finish(&self, app: &mut App) {
        assert!(
            app.is_plugin_added::<UiPlugin>(),
            "WorkbenchUiPlugin requires UiPlugin (either registration order)"
        );
        assert!(
            app.is_plugin_added::<WorkbenchCorePlugin>(),
            "WorkbenchUiPlugin requires WorkbenchCorePlugin"
        );
    }
}

/// The surface root owns error presentation, not any originating panel. Draw
/// the modal first so its layer and dismissal keys gate the rest of this pass.
pub(crate) fn draw_active(
    ui: &mut bedtime_ui::egui::Ui,
    world: &mut World,
    context: &bedtime_ui::UiContext<'_>,
) {
    // Errors first: a failed save may explain what the close prompt then asks about.
    let modal_this_frame = crate::dialogs::draw(ui.ctx(), world, context.surface)
        || crate::dialogs::draw_close_prompt(ui.ctx(), world, context.surface);
    // Keep the shell's ID scope stable whether or not a dialog is visible.
    let mut shell = bedtime_ui::egui::UiBuilder::new().id_salt("workbench-shell");
    if modal_this_frame {
        // egui's modal hit/focus observations use the previous pass. Explicitly
        // disable this pass's shell, including the first and closing frames.
        shell = shell.disabled();
    }
    ui.scope_builder(shell, |ui| {
        crate::frontend::draw_active(ui, world, context);
    });
}

fn observe_item(world: &mut World, entity: Entity, response: &bedtime_ui::egui::Response) {
    world.entity_mut(entity).insert(MenuPresentation {
        visible: true,
        rect: Rect::from_corners(
            Vec2::new(response.rect.min.x, response.rect.min.y),
            Vec2::new(response.rect.max.x, response.rect.max.y),
        ),
    });
}

/// Draw abstract menu entries followed by optional application content from a
/// UiView on the MenuBar entity. The renderer receives the menu root as its view.
pub fn draw_menu_bar(frame: &mut UiFrame, world: &mut World, root: Entity) -> Result<(), String> {
    #[cfg(feature = "native")]
    if world.get::<NativeMenuBar>(root).is_some() && native_menu_installed(world, root) {
        return Ok(());
    }
    let menu = menu_snapshot(world, root, InvocationContext::Focused)?;
    use bedtime_ui::egui;
    let available = frame.ui.available_rect_before_wrap();
    let padding = world
        .get::<WorkbenchSurface>(frame.surface)
        .copied()
        .unwrap_or_default()
        .panel_gap
        .min(available.width() * 0.5);
    let inner = available.shrink2(egui::vec2(padding, 0.0));
    let mut row = frame.ui.new_child(egui::UiBuilder::new().max_rect(inner));
    row.set_clip_rect(frame.ui.clip_rect().intersect(inner));
    let response = egui::MenuBar::new()
        .ui(&mut row, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.visuals_mut().widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
            draw_entries(ui, world, &menu.entries);
            if let Some(draw) = world.get::<bedtime_ui::UiView>(root).map(|view| view.draw) {
                draw(ui, world, &frame.context().for_view(root));
            }
        })
        .response;
    let bounds = egui::Rect::from_min_max(
        available.min,
        egui::pos2(available.max.x, response.rect.max.y),
    );
    frame.ui.advance_cursor_after_rect(bounds);
    frame.ui.painter().with_clip_rect(bounds).line_segment(
        [bounds.left_bottom(), bounds.right_bottom()],
        egui::Stroke::new(1.0, bedtime_ui::theme::color(frame.theme.palette.border)),
    );
    Ok(())
}

pub fn draw_popup_menu(
    _frame: &mut UiFrame,
    world: &mut World,
    response: &bedtime_ui::egui::Response,
    root: Entity,
    context: InvocationContext,
) -> Result<(), String> {
    let menu = menu_snapshot(world, root, context)?;
    response.context_menu(|ui| {
        draw_entries(ui, world, &menu.entries);
    });
    Ok(())
}

fn draw_entries(ui: &mut bedtime_ui::egui::Ui, world: &mut World, entries: &[MenuNode]) {
    use bedtime_ui::egui;
    for entry in entries {
        match entry {
            MenuNode::Menu {
                entity,
                label,
                entries,
            } => {
                let response = ui
                    .push_id(entity.to_bits(), |ui| {
                        ui.menu_button(label, |ui| draw_entries(ui, world, entries))
                    })
                    .inner
                    .response;
                observe_item(world, *entity, &response);
            }
            MenuNode::Item {
                entity,
                label,
                enabled,
                shortcut,
                target,
                context,
            } => {
                let text = shortcut
                    .map_or_else(|| label.clone(), |s| format!("{}     {}", label, s.label()));
                let response = ui
                    .push_id(entity.to_bits(), |ui| {
                        ui.add_enabled(*enabled, egui::Button::new(text))
                    })
                    .inner;
                observe_item(world, *entity, &response);
                if response.clicked() {
                    let request = ContextCommandRequest {
                        command: target.clone().expect("Enabled menu action"),
                        source: Some(*entity),
                        context: *context,
                    };
                    world
                        .resource_mut::<ContextCommandRequests>()
                        .submit(request);
                    ui.close();
                }
            }
            MenuNode::Separator(_) => {
                ui.separator();
            }
        }
    }
}
