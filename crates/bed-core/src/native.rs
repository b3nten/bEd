//! Native chrome projects the same contribution metadata consumed by the workbench.
use crate::{CorePresentationMode, CoreSystems};
use bed_api::workbench::{CommandPresentations, WorkbenchRegistry};
use bed_desktop::{
    DesktopMenu, DesktopMenuCommand, DesktopMenuItem, DesktopNativePresentation,
    DesktopNativeSystems, DesktopShortcut, DesktopSystems, DesktopTheme, DesktopToolbarItem,
};
use bevy::{prelude::*, window::PrimaryWindow};

pub(crate) fn install(app: &mut App) {
    app.add_systems(
        PreUpdate,
        publish
            .in_set(DesktopSystems::Prepare)
            .after(CoreSystems::Frame)
            .before(DesktopNativeSystems::Prepare),
    );
}

fn publish(world: &mut World) {
    if *world.resource::<CorePresentationMode>() != CorePresentationMode::Standard {
        return;
    }
    bed_workbench::resolve_keymap(world);
    let bindings = world
        .non_send::<bed_workbench::ResolvedKeymap>()
        .bindings
        .clone();
    let focused_scope = world
        .resource::<bed_workbench::WorkbenchLayout>()
        .focused
        .and_then(|panel| world.get::<bed_workbench::PanelRecord>(panel))
        .map(|record| bed_keybindings::Scope::panel(&record.panel_type));
    let shortcut = |id: &str| {
        bindings
            .for_action(id)
            .into_iter()
            .find(|binding| {
                let scope = focused_scope
                    .as_ref()
                    .unwrap_or(&bed_keybindings::Scope::Global);
                (binding.scope == bed_keybindings::Scope::Global
                    || focused_scope.as_ref() == Some(&binding.scope))
                    && bindings
                        .active_winner(scope, binding.shortcut)
                        .is_some_and(|winner| winner.action == id)
            })
            .map(|binding| DesktopShortcut {
                key: binding.shortcut.key,
                modifiers: bed_desktop::DesktopModifiers {
                    ctrl: binding.shortcut.modifiers.ctrl,
                    shift: binding.shortcut.modifiers.shift,
                    alt: binding.shortcut.modifiers.alt,
                    super_key: binding.shortcut.modifiers.super_key,
                },
            })
    };
    let name = world
        .query_filtered::<&Window, With<PrimaryWindow>>()
        .iter(world)
        .next()
        .map(|window| window.title.clone())
        .unwrap_or_else(|| "Application".into());
    let registry = world.resource::<WorkbenchRegistry>();
    let facts = world.get_resource::<CommandPresentations>();
    let mut items = registry
        .menus()
        .iter()
        .filter_map(|id| registry.command(id))
        .map(|command| command.presentation(facts))
        .filter(|command| command.visible)
        .map(|command| {
            DesktopMenuItem::Command(DesktopMenuCommand {
                id: command.id.into(),
                label: command.label.into(),
                enabled: command.enabled,
                shortcut: shortcut(command.id),
            })
        })
        .collect::<Vec<_>>();
    if !items.is_empty() {
        items.push(DesktopMenuItem::Separator);
    }
    items.push(DesktopMenuItem::Quit(DesktopMenuCommand {
        id: "bed.core.quit".into(),
        label: format!("Quit {name}"),
        enabled: true,
        shortcut: shortcut("bed.core.quit"),
    }));
    let toolbar = registry
        .toolbar()
        .iter()
        .filter_map(|id| registry.command(id))
        .map(|command| command.presentation(facts))
        .filter(|command| command.visible)
        .map(|command| DesktopToolbarItem {
            id: command.id.into(),
            label: command.label.into(),
            icon: command.icon.map(str::to_owned),
            enabled: command.enabled,
        })
        .collect();
    let appearance = world.resource::<DesktopTheme>().0.clone();
    let mut presentation = world.resource_mut::<DesktopNativePresentation>();
    presentation.menus = vec![DesktopMenu {
        label: name,
        items,
        windows_menu: false,
    }];
    presentation.toolbar = toolbar;
    presentation.opacity = appearance.opacity.clamp(0.0, 1.0);
    presentation.foreground = appearance.palette.text;
    presentation.background = appearance.palette.background;
}

#[cfg(test)]
mod tests {
    use super::*;
    use bed_api::workbench::{CommandInvocation, CommandPresentation};

    #[test]
    fn native_chrome_uses_shared_visibility_label_icon_availability_and_theme() {
        let mut app = App::new();
        app.init_resource::<CorePresentationMode>()
            .init_resource::<DesktopTheme>()
            .init_resource::<bed_workbench::WorkbenchLayout>()
            .init_resource::<DesktopNativePresentation>()
            .init_resource::<CommandPresentations>();
        bed_api::workbench::register(app.world_mut(), "example", |registrar| {
            for id in ["example.open", "example.hidden"] {
                registrar.command(id, "Open", |_: In<CommandInvocation>| Ok(Vec::new()));
                registrar.command_icon(id, "example.open-icon");
                registrar.menu(id);
                registrar.toolbar(id);
            }
        })
        .unwrap();
        app.world_mut().spawn((
            Window {
                title: "Example".into(),
                ..Default::default()
            },
            PrimaryWindow,
        ));
        {
            let mut facts = app.world_mut().resource_mut::<CommandPresentations>();
            facts.0.insert(
                "example.open".into(),
                CommandPresentation {
                    label: Some("Working…".into()),
                    enabled: false,
                    visible: true,
                },
            );
            facts.0.insert(
                "example.hidden".into(),
                CommandPresentation {
                    visible: false,
                    ..Default::default()
                },
            );
        }
        {
            let mut appearance = app.world_mut().resource_mut::<DesktopTheme>();
            appearance.0.palette.text = [0.2, 0.3, 0.4, 1.0];
            appearance.0.opacity = 0.5;
        }
        install(&mut app);
        app.update();
        let native = app.world().resource::<DesktopNativePresentation>();
        assert_eq!(native.toolbar.len(), 1);
        assert_eq!(native.toolbar[0].id, "example.open");
        assert_eq!(native.toolbar[0].label, "Working…");
        assert_eq!(native.toolbar[0].icon.as_deref(), Some("example.open-icon"));
        assert!(!native.toolbar[0].enabled);
        assert_eq!(native.foreground, [0.2, 0.3, 0.4, 1.0]);
        assert_eq!(native.opacity, 0.5);
        assert!(
            matches!(&native.menus[0].items[0], DesktopMenuItem::Command(command) if command.id == "example.open" && command.label == "Working…" && !command.enabled)
        );
        let saved = native.clone();
        *app.world_mut().resource_mut::<CorePresentationMode>() = CorePresentationMode::Application;
        app.world_mut()
            .resource_mut::<CommandPresentations>()
            .0
            .clear();
        app.update();
        assert_eq!(*app.world().resource::<DesktopNativePresentation>(), saved);
    }
    #[test]
    fn native_accelerators_follow_live_preference_remaps_and_unbinding() {
        use bed_api::settings::SettingsRequest;
        let mut app = App::new();
        app.init_resource::<CorePresentationMode>()
            .init_resource::<DesktopTheme>()
            .init_resource::<bed_workbench::WorkbenchLayout>()
            .init_resource::<DesktopNativePresentation>();
        bed_api::workbench::register(app.world_mut(), "fixture", |r| {
            for id in ["fixture.a", "fixture.b"] {
                r.command(id, id, |_: In<CommandInvocation>| Ok(Vec::new()));
                r.global_shortcut(id, &["Primary+S"]);
                r.menu(id);
            }
        })
        .unwrap();
        crate::settings::initialize_settings(app.world_mut(), None).unwrap();
        crate::settings::finalize_settings(app.world_mut()).unwrap();
        let accelerator = |world: &World, id: &str| {
            world.resource::<DesktopNativePresentation>().menus[0]
                .items
                .iter()
                .find_map(|item| match item {
                    DesktopMenuItem::Command(command) if command.id == id => {
                        Some(command.shortcut.clone())
                    }
                    _ => None,
                })
                .unwrap()
        };
        publish(app.world_mut());
        assert!(accelerator(app.world(), "fixture.a").is_none());
        assert!(accelerator(app.world(), "fixture.b").is_some());
        crate::settings::apply_settings_request(
            app.world_mut(),
            &SettingsRequest::new(
                "bed.workbench",
                "keybindings",
                serde_json::json!({
                    "version": 1, "actions": {"fixture.a": [{"shortcut": "Primary+R", "order": 1}]}
                }),
            ),
        )
        .unwrap();
        publish(app.world_mut());
        assert_eq!(
            accelerator(app.world(), "fixture.a").unwrap().key,
            dear_imgui_rs::Key::R
        );
        assert_eq!(
            accelerator(app.world(), "fixture.b").unwrap().key,
            dear_imgui_rs::Key::S
        );
        let map = app
            .world()
            .non_send::<bed_workbench::ResolvedKeymap>()
            .bindings
            .clone();
        publish(app.world_mut());
        assert!(std::rc::Rc::ptr_eq(
            &map,
            &app.world()
                .non_send::<bed_workbench::ResolvedKeymap>()
                .bindings
        ));
        crate::settings::apply_settings_request(
            app.world_mut(),
            &SettingsRequest::new(
                "bed.workbench",
                "keybindings",
                serde_json::json!({
                    "version": 1, "actions": {"fixture.b": []}
                }),
            ),
        )
        .unwrap();
        publish(app.world_mut());
        assert_eq!(
            accelerator(app.world(), "fixture.a").unwrap().key,
            dear_imgui_rs::Key::S
        );
        assert!(accelerator(app.world(), "fixture.b").is_none());
    }
}
