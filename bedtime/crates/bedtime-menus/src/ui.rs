//! Drawing menus with egui: a menu bar, or a context menu.
use crate::*;
use bedtime_actions::ActionRequests;
use bedtime_ui::{UiFrame, egui};

/// Where an entry was last drawn, for tests and automation. Native menus are
/// independent.
#[derive(Component, Default, Clone, Copy)]
pub struct MenuPresentation {
    pub visible: bool,
    pub rect: Rect,
}

fn observe_item(world: &mut World, entity: Entity, response: &egui::Response) {
    world.entity_mut(entity).insert(MenuPresentation {
        visible: true,
        rect: Rect::from_corners(
            Vec2::new(response.rect.min.x, response.rect.min.y),
            Vec2::new(response.rect.max.x, response.rect.max.y),
        ),
    });
}

/// Draw a menu bar's entries, followed by optional application content from a
/// `UiView` on the `MenuBar` entity (which it receives as its view), with
/// `inset` points of space at each side. Draws nothing while the native menu
/// bar shows this root.
pub fn draw_menu_bar(
    frame: &mut UiFrame,
    world: &mut World,
    root: Entity,
    inset: f32,
) -> Result<(), String> {
    #[cfg(feature = "native")]
    if world.get::<NativeMenuBar>(root).is_some() && native::installed(world, root) {
        return Ok(());
    }
    let menu = world.menus().snapshot(root, InvocationContext::Focused)?;
    let available = frame.ui.available_rect_before_wrap();
    let padding = inset.min(available.width() * 0.5);
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
    response: &egui::Response,
    root: Entity,
    context: InvocationContext,
) -> Result<(), String> {
    let menu = world.menus().snapshot(root, context)?;
    response.context_menu(|ui| {
        draw_entries(ui, world, &menu.entries);
    });
    Ok(())
}

fn draw_entries(ui: &mut egui::Ui, world: &mut World, entries: &[MenuNode]) {
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
                    let request = ActionRequest {
                        action: target.clone().expect("Enabled menu action"),
                        context: *context,
                        source: Some(*entity),
                    };
                    world.resource_mut::<ActionRequests>().submit(request);
                    ui.close();
                }
            }
            MenuNode::Separator(_) => {
                ui.separator();
            }
        }
    }
}
