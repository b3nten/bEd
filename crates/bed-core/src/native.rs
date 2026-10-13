//! Native chrome projects the same contribution metadata consumed by the workbench.
use crate::{CoreSystems, HostRequestMode};
use bed_api::workbench::{CommandPresentations, WorkbenchRegistry};
use bed_desktop::{
    DesktopAppearance, DesktopMenu, DesktopMenuCommand, DesktopMenuItem, DesktopNativePresentation,
    DesktopNativeSystems, DesktopShortcut, DesktopSystems, DesktopToolbarItem,
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
    if *world.resource::<HostRequestMode>() != HostRequestMode::Core {
        return;
    }
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
                shortcut: None,
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
        shortcut: Some(DesktopShortcut {
            key: dear_imgui_rs::Key::Q,
            modifiers: bed_desktop::DesktopModifiers {
                super_key: cfg!(target_os = "macos"),
                ctrl: !cfg!(target_os = "macos"),
                ..Default::default()
            },
        }),
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
    let appearance = world.resource::<DesktopAppearance>().0.clone();
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
        app.init_resource::<HostRequestMode>()
            .init_resource::<DesktopAppearance>()
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
            let mut appearance = app.world_mut().resource_mut::<DesktopAppearance>();
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
        *app.world_mut().resource_mut::<HostRequestMode>() = HostRequestMode::Application;
        app.world_mut()
            .resource_mut::<CommandPresentations>()
            .0
            .clear();
        app.update();
        assert_eq!(*app.world().resource::<DesktopNativePresentation>(), saved);
    }
}
