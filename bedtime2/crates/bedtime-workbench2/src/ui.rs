//! Optional UI presentation and input observations. UI2 has no workbench dependency.
use crate::*;
use bedtime_ui2::{UiFrame, UiPlugin, UiSystems};
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
            frontend::sync_viewports
                .after(WorkspaceSystems::Prepare)
                .before(UiSystems::Draw),
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

fn observe_item(world: &mut World, entity: Entity, response: &bedtime_ui2::egui::Response) {
    world.entity_mut(entity).insert(MenuPresentation {
        visible: true,
        rect: Rect::from_corners(
            Vec2::new(response.rect.min.x, response.rect.min.y),
            Vec2::new(response.rect.max.x, response.rect.max.y),
        ),
    });
}

pub fn draw_menu_bar(frame: &mut UiFrame, world: &mut World, root: Entity) -> Result<(), String> {
    #[cfg(feature = "native")]
    if world.get::<NativeMenuBar>(root).is_some()
        && world
            .get_resource::<NativeCapabilities>()
            .is_some_and(|c| c.native_menus)
    {
        return Ok(());
    }
    let menu = menu_snapshot(world, root, InvocationContext::Focused)?;
    use bedtime_ui2::egui;
    let available = frame.ui.available_rect_before_wrap();
    let padding = frame.panel_settings.panel_gap.min(available.width() * 0.5);
    let inner = available.shrink2(egui::vec2(padding, 0.0));
    let mut row = frame.ui.new_child(egui::UiBuilder::new().max_rect(inner));
    row.set_clip_rect(frame.ui.clip_rect().intersect(inner));
    let response = egui::MenuBar::new()
        .ui(&mut row, |ui| draw_entries(ui, world, &menu.entries))
        .response;
    let bounds = egui::Rect::from_min_max(
        available.min,
        egui::pos2(available.max.x, response.rect.max.y),
    );
    frame.ui.advance_cursor_after_rect(bounds);
    frame.ui.painter().with_clip_rect(bounds).line_segment(
        [bounds.left_bottom(), bounds.right_bottom()],
        egui::Stroke::new(1.0, bedtime_ui2::theme::color(frame.theme.palette.border)),
    );
    Ok(())
}

pub fn draw_popup_menu(
    _frame: &mut UiFrame,
    world: &mut World,
    response: &bedtime_ui2::egui::Response,
    root: Entity,
    context: InvocationContext,
) -> Result<(), String> {
    let menu = menu_snapshot(world, root, context)?;
    response.context_menu(|ui| {
        draw_entries(ui, world, &menu.entries);
    });
    Ok(())
}

fn draw_entries(ui: &mut bedtime_ui2::egui::Ui, world: &mut World, entries: &[MenuNode]) {
    use bedtime_ui2::egui;
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
                    world
                        .resource_mut::<ContextCommandRequests>()
                        .submit(ContextCommandRequest {
                            command: target.clone().expect("Enabled menu action"),
                            source: Some(*entity),
                            context: *context,
                        });
                    ui.close();
                }
            }
            MenuNode::Separator(_) => {
                ui.separator();
            }
        }
    }
}
