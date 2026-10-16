//! Optional UI presentation and input observations. UI2 has no workbench dependency.
use crate::*;
use bedtime_ui2::{UiCapture, UiFrame, UiPlugin, UiSystems};
use bevy::prelude::*;

/// Screen observations of the ImGui presentation. Native menus remain independent.
#[derive(Component, Default, Clone, Copy)]
pub struct MenuPresentation {
    pub visible: bool,
    pub rect: Rect,
}

pub struct WorkbenchUiPlugin;
impl Plugin for WorkbenchUiPlugin {
    fn build(&self, app: &mut App) {
        frontend::configure(app);
        app.add_systems(PostUpdate, reset_menus.before(UiSystems::Draw));
        app.add_systems(
            PostUpdate,
            observe
                .after(UiSystems::Draw)
                .before(CommandSystems::Shortcuts),
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

fn reset_menus(mut menus: Query<&mut MenuPresentation>) {
    for mut menu in &mut menus {
        menu.visible = false;
    }
}

fn observe_item(frame: &UiFrame, world: &mut World, entity: Entity) {
    world.entity_mut(entity).insert(MenuPresentation {
        visible: frame.ui.is_item_visible(),
        rect: Rect::from_corners(
            Vec2::from_array(frame.ui.item_rect_min()),
            Vec2::from_array(frame.ui.item_rect_max()),
        ),
    });
}

fn observe(world: &mut World) {
    let (keyboard, text) = world
        .query::<&UiCapture>()
        .iter(world)
        .fold((false, false), |(k, t), c| (k || c.keyboard, t || c.text));
    let mut capture = world.resource_mut::<ShortcutCapture>();
    capture.keyboard = keyboard;
    capture.text = text;
    #[cfg(feature = "native")]
    {
        use bedtime_ui2::{Theme, UiSurface, UsesTheme};
        let backgrounds: Vec<_> = world
            .query_filtered::<(Entity, &UsesTheme), With<UiSurface>>()
            .iter(world)
            .map(|(e, t)| {
                (
                    e,
                    WindowBackground::new(
                        world
                            .get::<Theme>(t.0)
                            .expect("Surface theme")
                            .palette
                            .background,
                    ),
                )
            })
            .collect();
        for (entity, background) in backgrounds {
            if world.get::<WindowBackground>(entity) != Some(&background) {
                world.entity_mut(entity).insert(background);
            }
        }
    }
}

/// Render an abstract MenuBar inside an ImGui window with MENU_BAR enabled.
pub fn draw_menu_bar(frame: &UiFrame, world: &mut World, root: Entity) -> Result<(), String> {
    let menu = menu_snapshot(world, root, CommandContext::Focused)?;
    if let Some(_bar) = frame.ui.begin_menu_bar() {
        draw_entries(frame, world, &menu.entries);
    }
    Ok(())
}

/// Call after the source control. Popup styling and animation are supplied by UI2.
pub fn draw_popup_menu(
    frame: &UiFrame,
    world: &mut World,
    id: &str,
    root: Entity,
    context: CommandContext,
) -> Result<(), String> {
    let mut result = Ok(());
    frame.controls().context_menu(id, || {
        result = menu_snapshot(world, root, context)
            .map(|menu| draw_entries(frame, world, &menu.entries));
    });
    result
}

fn draw_entries(frame: &UiFrame, world: &mut World, entries: &[MenuNode]) {
    for entry in entries {
        match entry {
            MenuNode::Menu {
                entity,
                label,
                entries,
            } => {
                let _id = frame.ui.push_id(entity.to_bits() as usize);
                let menu = frame.ui.begin_menu(label);
                observe_item(frame, world, *entity);
                if let Some(_menu) = menu {
                    draw_entries(frame, world, entries);
                }
            }
            MenuNode::Item {
                entity,
                label,
                enabled,
                shortcut,
                target,
                context,
            } => {
                let _id = frame.ui.push_id(entity.to_bits() as usize);
                let shortcut = shortcut.map(|s| s.label()).unwrap_or_default();
                let clicked = frame
                    .ui
                    .menu_item_enabled_selected_with_shortcut(label, &shortcut, false, *enabled);
                observe_item(frame, world, *entity);
                if clicked {
                    world
                        .resource_mut::<CommandRequests>()
                        .submit(CommandRequest {
                            command: target.expect("Enabled menu action"),
                            source: Some(*entity),
                            context: *context,
                        });
                }
            }
            MenuNode::Separator(_) => frame.ui.separator(),
        }
    }
}
