//! Native chrome projects the same contribution metadata consumed by the workbench.
use crate::CoreSystems;
use bedtime_api::workbench::{CommandPresentations, WorkbenchRegistry};
use bedtime_desktop::{
    DesktopNativePresentation, DesktopNativeSystems, DesktopSystems, DesktopTheme,
    DesktopToolbarItem,
};
use bevy::prelude::*;

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
    bedtime_workbench::resolve_keymap(world);
    let menus = match crate::resolve_menus(world) {
        Ok(menus) => menus,
        Err(error) => {
            world
                .resource_mut::<bedtime_desktop::DesktopNativeErrors>()
                .0
                .push(error);
            Vec::new()
        }
    };
    let registry = world.resource::<WorkbenchRegistry>();
    let facts = world.get_resource::<CommandPresentations>();
    let toolbar = registry
        .toolbar()
        .iter()
        .filter_map(|id| registry.command(id))
        .map(|command| command.presentation(facts))
        .filter(|command| command.visible)
        .map(|command| DesktopToolbarItem {
            id: command.id.to_string(),
            label: command.label.into(),
            icon: command.icon.map(str::to_owned),
            enabled: command.enabled,
        })
        .collect();
    let appearance = world.resource::<DesktopTheme>().0.clone();
    let mut presentation = world.resource_mut::<DesktopNativePresentation>();
    presentation.menus = menus;
    presentation.toolbar = toolbar;
    presentation.opacity = appearance.opacity.clamp(0.0, 1.0);
    presentation.foreground = appearance.palette.text;
    presentation.background = appearance.palette.background;
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedtime_api::workbench::{CommandInvocation, CommandPresentation};
    use bedtime_desktop::DesktopMenuItem;
    use bevy::window::PrimaryWindow;

    #[test]
    fn native_chrome_uses_shared_visibility_label_icon_availability_and_theme() {
        let mut app = App::new();
        app.init_resource::<DesktopTheme>()
            .init_resource::<bedtime_workbench::WorkbenchLayout>()
            .init_resource::<DesktopNativePresentation>()
            .init_resource::<CommandPresentations>();
        bedtime_api::workbench::register(app.world_mut(), "example", |registrar| {
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
                    ..Default::default()
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
        app.insert_resource(bedtime_api::menus::MenuLayout(vec![]));
        app.world_mut()
            .resource_mut::<CommandPresentations>()
            .0
            .clear();
        app.update();
        assert!(
            app.world()
                .resource::<DesktopNativePresentation>()
                .menus
                .is_empty()
        );
        assert_eq!(
            app.world()
                .resource::<DesktopNativePresentation>()
                .foreground,
            saved.foreground
        );
    }
    #[test]
    fn native_accelerators_follow_live_preference_remaps_and_unbinding() {
        use bedtime_api::settings::SettingsRequest;
        let mut app = App::new();
        app.init_resource::<DesktopTheme>()
            .init_resource::<bedtime_workbench::WorkbenchLayout>()
            .init_resource::<DesktopNativePresentation>();
        bedtime_api::workbench::register(app.world_mut(), "fixture", |r| {
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
                "bedtime.workbench",
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
            .non_send::<bedtime_workbench::ResolvedKeymap>()
            .bindings
            .clone();
        publish(app.world_mut());
        assert!(std::rc::Rc::ptr_eq(
            &map,
            &app.world()
                .non_send::<bedtime_workbench::ResolvedKeymap>()
                .bindings
        ));
        crate::settings::apply_settings_request(
            app.world_mut(),
            &SettingsRequest::new(
                "bedtime.workbench",
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
