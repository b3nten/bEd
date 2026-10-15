//! Resolve one application menu tree for native and ImGui presentation.
use bedtime_api::{
    menus::{Menu, MenuCommand, MenuEntry, MenuLayout},
    workbench::{CommandPresentations, WorkbenchRegistry},
};
use bedtime_desktop::{
    DesktopMenu, DesktopMenuCommand, DesktopMenuItem, DesktopModifiers, DesktopShortcut,
};
use bevy::{prelude::*, window::PrimaryWindow};

pub fn menu_shortcut(world: &World, id: &str) -> Option<bedtime_keybindings::Stroke> {
    let bindings = &world
        .get_non_send::<bedtime_workbench::ResolvedKeymap>()?
        .bindings;
    let focused_scope = world
        .resource::<bedtime_workbench::WorkbenchLayout>()
        .focused
        .and_then(|panel| world.get::<bedtime_workbench::PanelRecord>(panel))
        .map(|record| bedtime_keybindings::Scope::panel(record.panel_type.as_str()));
    if world
        .get_non_send::<bedtime_desktop::DesktopContext>()
        .is_some_and(|context| context.context.io().want_text_input())
        && let Some(action) = crate::NativeEditAction::from_command(id)
    {
        return Some(action.shortcut());
    }
    let scope = focused_scope
        .as_ref()
        .unwrap_or(&bedtime_keybindings::Scope::Global);
    bindings
        .for_action(id)
        .into_iter()
        .find(|binding| {
            (binding.scope == bedtime_keybindings::Scope::Global
                || focused_scope.as_ref() == Some(&binding.scope)
                || binding.scope == bedtime_keybindings::Scope::Focused && focused_scope.is_some())
                && bindings
                    .active_winner(scope, binding.shortcut)
                    .is_some_and(|winner| winner.action == id)
        })
        .map(|binding| binding.shortcut)
}

fn command(world: &World, entry: &MenuCommand) -> Option<DesktopMenuCommand> {
    let definition = world.resource::<WorkbenchRegistry>().command(&entry.id)?;
    let presentation = definition.presentation(world.get_resource::<CommandPresentations>());
    if !presentation.visible {
        return None;
    }
    Some(DesktopMenuCommand {
        id: entry.id.to_string(),
        label: entry
            .label
            .clone()
            .unwrap_or_else(|| presentation.label.into()),
        enabled: presentation.enabled,
        shortcut: menu_shortcut(world, &entry.id).map(|stroke| DesktopShortcut {
            key: stroke.key,
            modifiers: DesktopModifiers {
                ctrl: stroke.modifiers.ctrl,
                shift: stroke.modifiers.shift,
                alt: stroke.modifiers.alt,
                super_key: stroke.modifiers.super_key,
            },
        }),
    })
}

fn resolve_entries(world: &World, entries: &[MenuEntry]) -> Vec<DesktopMenuItem> {
    let mut resolved = Vec::new();
    for entry in entries {
        let item = match entry {
            MenuEntry::Command(entry) => command(world, entry).map(|command| {
                if entry.selected {
                    DesktopMenuItem::SelectedCommand(command)
                } else {
                    DesktopMenuItem::Command(command)
                }
            }),
            MenuEntry::Quit(entry) => command(world, entry).map(DesktopMenuItem::Quit),
            MenuEntry::Submenu(menu) => {
                let items = resolve_entries(world, &menu.entries);
                (!items.is_empty()).then(|| DesktopMenuItem::Submenu {
                    label: menu.label.clone(),
                    items,
                })
            }
            MenuEntry::Contributions => {
                if let Some(features) =
                    world.get_resource::<bedtime_api::features::FeatureRegistry>()
                {
                    resolved.extend(
                        features
                            .menus()
                            .iter()
                            .filter(|entry| {
                                entry.slot == bedtime_api::features::MenuSlot::Application
                            })
                            .filter_map(|entry| {
                                command(world, &MenuCommand::new(entry.command.clone()))
                                    .map(DesktopMenuItem::Command)
                            }),
                    );
                }
                None
            }
            MenuEntry::Separator => Some(DesktopMenuItem::Separator),
            MenuEntry::About {
                label,
                name,
                version,
                comments,
                credits,
            } => Some(DesktopMenuItem::About {
                label: label.clone(),
                name: name.clone(),
                version: version.clone(),
                comments: comments.clone(),
                credits: credits.clone(),
            }),
            MenuEntry::Services => Some(DesktopMenuItem::Services),
            MenuEntry::Hide(label) => Some(DesktopMenuItem::Hide(label.clone())),
            MenuEntry::HideOthers => Some(DesktopMenuItem::HideOthers),
            MenuEntry::ShowAll => Some(DesktopMenuItem::ShowAll),
            MenuEntry::Minimize => Some(DesktopMenuItem::Minimize),
            MenuEntry::Maximize => Some(DesktopMenuItem::Maximize),
            MenuEntry::BringAllToFront => Some(DesktopMenuItem::BringAllToFront),
        };
        if let Some(item) = item {
            if matches!(item, DesktopMenuItem::Separator)
                && (resolved.is_empty()
                    || matches!(resolved.last(), Some(DesktopMenuItem::Separator)))
            {
                continue;
            }
            resolved.push(item);
        }
    }
    while matches!(resolved.last(), Some(DesktopMenuItem::Separator)) {
        resolved.pop();
    }
    resolved
}

pub fn resolve_menus(world: &mut World) -> Result<Vec<DesktopMenu>, String> {
    let layout = if let Some(layout) = world.get_resource::<MenuLayout>().cloned() {
        layout
    } else {
        let name = world
            .query_filtered::<&Window, With<PrimaryWindow>>()
            .iter(world)
            .next()
            .map(|window| window.title.clone())
            .unwrap_or_else(|| "Application".into());
        let mut ids = world.resource::<WorkbenchRegistry>().menus().to_vec();
        if let Some(features) = world.get_resource::<bedtime_api::features::FeatureRegistry>() {
            for contribution in features
                .menus()
                .iter()
                .filter(|entry| entry.slot == bedtime_api::features::MenuSlot::Application)
            {
                if !ids.contains(&contribution.command) {
                    ids.push(contribution.command.clone());
                }
            }
        }
        let mut entries = ids
            .into_iter()
            .map(|id| MenuEntry::Command(MenuCommand::new(id)))
            .collect::<Vec<_>>();
        entries.push(MenuEntry::Separator);
        entries.push(MenuEntry::Quit(
            MenuCommand::new("bedtime.core.quit").labeled(format!("Quit {name}")),
        ));
        MenuLayout(vec![Menu::new(name, entries)])
    };
    layout.validate()?;
    Ok(layout
        .0
        .iter()
        .filter_map(|menu| {
            let items = resolve_entries(world, &menu.entries);
            (!items.is_empty()).then(|| DesktopMenu {
                label: menu.label.clone(),
                items,
                windows_menu: menu.windows_menu,
            })
        })
        .collect())
}

pub(crate) fn draw_menus(world: &mut World, ui: &dear_imgui_rs::Ui) -> Result<(), String> {
    fn draw(world: &mut World, ui: &dear_imgui_rs::Ui, items: &[DesktopMenuItem]) {
        for item in items {
            match item {
                DesktopMenuItem::Command(command)
                | DesktopMenuItem::SelectedCommand(command)
                | DesktopMenuItem::Quit(command) => {
                    if ui.menu_item_enabled_selected_no_shortcut(
                        &command.label,
                        matches!(item, DesktopMenuItem::SelectedCommand(_)),
                        command.enabled,
                    ) {
                        let id = bedtime_api::CommandId::from(&command.id);
                        let arguments = world
                            .get_resource::<MenuLayout>()
                            .map_or_else(
                                || Ok(serde_json::Value::Null),
                                |layout| layout.arguments(&id),
                            )
                            .expect("resolved menu payloads are valid");
                        let mut invocation =
                            bedtime_workbench::capture_command(world, id, arguments);
                        if matches!(item, DesktopMenuItem::Quit(_)) {
                            invocation.source = None;
                            world
                                .resource_mut::<bedtime_workbench::WorkbenchRequests>()
                                .push_unscoped(bedtime_api::workbench::WorkbenchAction::Invoke(
                                    invocation,
                                ));
                        } else {
                            world
                                .resource_mut::<bedtime_workbench::WorkbenchRequests>()
                                .push(bedtime_api::workbench::WorkbenchAction::Invoke(invocation));
                        }
                    }
                }
                DesktopMenuItem::Submenu { label, items } => {
                    if let Some(_menu) = ui.begin_menu(label) {
                        draw(world, ui, items);
                    }
                }
                DesktopMenuItem::Separator => ui.separator(),
                _ => {}
            }
        }
    }
    if world
        .get_resource::<bedtime_desktop::DesktopNativeCapabilities>()
        .is_some_and(|native| native.menus)
    {
        return Ok(());
    }
    let menus = resolve_menus(world)?;
    if let Some(_bar) = ui.begin_main_menu_bar() {
        for menu in menus {
            if let Some(_menu) = ui.begin_menu(&menu.label) {
                draw(world, ui, &menu.items);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedtime_api::workbench::{CommandInvocation, CommandPresentation, WorkbenchAction};
    use bedtime_desktop::DesktopNativeActions;
    use serde_json::json;

    #[derive(Resource, Default)]
    struct Captured(Vec<CommandInvocation>);

    fn application() -> App {
        let mut app = App::new();
        app.add_plugins(bedtime_workbench::WorkbenchPlugin)
            .add_message::<crate::NativeCommandDispatched>()
            .init_resource::<DesktopNativeActions>()
            .init_resource::<Captured>();
        bedtime_api::workbench::register(app.world_mut(), "example", |registrar| {
            for id in ["example.open", "example.hidden"] {
                registrar.command(
                    id,
                    "Open",
                    |In(invocation): In<CommandInvocation>, mut captured: ResMut<Captured>| {
                        captured.0.push(invocation);
                        Ok(Vec::<WorkbenchAction>::new())
                    },
                );
            }
            registrar.global_shortcut("example.open", &["Primary+O"]);
            registrar.menu("example.open");
        })
        .unwrap();
        app
    }

    #[test]
    fn custom_menu_resolves_live_labels_availability_and_optional_plugins() {
        let mut app = application();
        app.insert_resource(MenuLayout(vec![Menu::new(
            "Database",
            vec![
                MenuEntry::Separator,
                MenuEntry::Command(MenuCommand::new("example.open")),
                MenuEntry::Command(MenuCommand::new("missing.optional")),
                MenuEntry::Submenu(Menu::new(
                    "Unused",
                    vec![MenuEntry::Command(MenuCommand::new("example.hidden"))],
                )),
                MenuEntry::Separator,
            ],
        )]));
        app.world_mut()
            .resource_mut::<CommandPresentations>()
            .0
            .insert(
                "example.open".into(),
                CommandPresentation {
                    label: Some("Connecting…".into()),
                    enabled: false,
                    ..Default::default()
                },
            );
        app.world_mut()
            .resource_mut::<CommandPresentations>()
            .0
            .insert(
                "example.hidden".into(),
                CommandPresentation {
                    visible: false,
                    ..Default::default()
                },
            );
        bedtime_workbench::resolve_keymap(app.world_mut());
        let menu = resolve_menus(app.world_mut()).unwrap().pop().unwrap();
        assert_eq!(menu.label, "Database");
        assert_eq!(menu.items.len(), 1);
        let DesktopMenuItem::Command(command) = &menu.items[0] else {
            panic!("visible registered command")
        };
        assert_eq!(command.label, "Connecting…");
        assert!(!command.enabled);
        assert!(command.shortcut.is_some());
    }

    #[test]
    fn native_click_preserves_application_arguments_and_runs_the_registered_owner() {
        let mut app = application();
        app.insert_resource(MenuLayout(vec![Menu::new(
            "Database",
            vec![MenuEntry::Command(
                MenuCommand::new("example.open").with_arguments(json!({"action":"open_folder"})),
            )],
        )]));
        app.world_mut()
            .resource_mut::<DesktopNativeActions>()
            .0
            .push(bedtime_desktop::DesktopNativeAction {
                command: "example.open".into(),
                keyboard: false,
                modifiers: Default::default(),
                quit: false,
            });
        crate::route_native_commands(app.world_mut());
        bedtime_workbench::process_requests(app.world_mut());
        let captured = &app.world().resource::<Captured>().0;
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].command, "example.open");
        assert_eq!(captured[0].arguments, json!({"action":"open_folder"}));
    }

    #[test]
    fn default_menu_includes_plugin_contributions_once() {
        let mut app = application();
        app.init_resource::<bedtime_api::features::FeatureRegistry>();
        app.world_mut()
            .resource_mut::<bedtime_api::features::FeatureRegistry>()
            .register_with("example", |registrar| {
                registrar.command_ref("example.open");
                registrar.command_ref("example.hidden");
                registrar.menu(bedtime_api::features::MenuSlot::Application, "example.open");
                registrar.menu(
                    bedtime_api::features::MenuSlot::Application,
                    "example.hidden",
                );
            })
            .unwrap();
        let menu = resolve_menus(app.world_mut()).unwrap().pop().unwrap();
        let ids = menu
            .items
            .iter()
            .filter_map(|item| match item {
                DesktopMenuItem::Command(command) => Some(command.id.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(ids, ["example.open", "example.hidden"]);
    }
}
