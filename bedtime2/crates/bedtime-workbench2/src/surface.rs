//! Default shell wiring, with explicit entities remaining the source of truth.
use crate::*;
use bedtime_ui2::{InView, UiBackground, UiCamera, UiSurface, UsesTheme, register_view, themes};
use bevy::{camera::RenderTarget, prelude::*, window::WindowRef};

/// Workbench configuration on a window, rendered by an explicit WorkbenchCamera.
/// Supply a theme or WorkbenchShell alongside this to override their defaults.
/// Wiring runs in PreUpdate before bedtime_ui2::UiSystems::Configure.
/// User permissions do not restrict direct application layout operations.
#[derive(Component, Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[component(on_remove = teardown)]
#[require(UiSurface, WorkbenchShell, UiBackground::Transparent)]
pub struct WorkbenchSurface {
    pub rounding: f32,
    /// Suggested opt-in content padding; panel hosts provide the complete area.
    pub padding: [f32; 2],
    /// Spacing in egui points between panels and from panels to shell edges.
    pub panel_gap: f32,
    /// Corner-target size, measured inward from the visible panel corner after gaps.
    /// Zero disables corner handles.
    pub hit_size: f32,
    pub debug: bool,
    /// Suppress panel borders, rounding and gaps when the layout has one area.
    pub single_area_edge_to_edge: bool,
    pub tab_bar: TabBarVisibility,
    /// Enable corner handles for splitting, joining and swapping areas.
    /// Disabling them leaves divider resizing and tab dragging available.
    pub drag_corners: bool,
    pub layout_editing: bool,
    pub panel_creation: bool,
    pub panel_closure: bool,
}

impl Default for WorkbenchSurface {
    fn default() -> Self {
        Self {
            rounding: 7.0,
            padding: [14.0, 12.0],
            panel_gap: 5.0,
            hit_size: 13.0,
            debug: false,
            single_area_edge_to_edge: true,
            tab_bar: TabBarVisibility::Always,
            drag_corners: true,
            layout_editing: true,
            panel_creation: true,
            panel_closure: true,
        }
    }
}

impl WorkbenchSurface {
    /// A fixed, edge-to-edge panel without tabs or user layout manipulation.
    pub fn single_panel() -> Self {
        Self {
            tab_bar: TabBarVisibility::Hidden,
            drag_corners: false,
            layout_editing: false,
            panel_creation: false,
            panel_closure: false,
            ..default()
        }
    }
}

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TabBarVisibility {
    #[default]
    Always,
    MultipleTabs,
    Hidden,
}

/// Required application-owned camera. UiCamera (and its EguiContext) is supplied
/// immediately; surface configuration resolves its initially unbound target.
#[derive(Component)]
#[require(UiCamera(Entity::PLACEHOLDER))]
pub struct WorkbenchCamera;

/// Only the resources and camera changes installed by workbench setup.
#[derive(Component, Clone)]
struct SurfaceBinding {
    root: Entity,
    camera: Entity,
    managed_ui_camera: bool,
    original_target: RenderTarget,
    quit_shortcuts: Vec<bedtime_ui2::egui::KeyboardShortcut>,
    generated_theme: Option<Entity>,
    #[cfg(feature = "native")]
    original_background: Option<WindowBackground>,
}

fn teardown(
    mut world: bevy::ecs::world::DeferredWorld,
    context: bevy::ecs::lifecycle::HookContext,
) {
    // Capture before despawning the window also removes its binding component.
    let Some(binding) = world.get::<SurfaceBinding>(context.entity).cloned() else {
        return;
    };
    world.commands().queue(move |world: &mut World| {
        let surface = context.entity;
        if world.get_entity(binding.root).is_ok() {
            world.despawn(binding.root);
        }
        if let Some(theme) = binding.generated_theme {
            if world
                .get::<UsesTheme>(surface)
                .is_some_and(|uses| uses.0 == theme)
            {
                world.entity_mut(surface).remove::<UsesTheme>();
            }
            if world.get_entity(theme).is_ok() {
                world.despawn(theme);
            }
        }
        if let Some(mut context) = world.get_mut::<bevy_egui::EguiContext>(binding.camera) {
            context
                .get_mut()
                .options_mut(|options| options.quit_shortcuts = binding.quit_shortcuts);
        }
        if world
            .get::<UiCamera>(binding.camera)
            .is_some_and(|camera| camera.0 == surface)
        {
            if binding.managed_ui_camera || world.get_entity(surface).is_err() {
                world.entity_mut(binding.camera).remove::<UiCamera>();
            }
            world
                .entity_mut(binding.camera)
                .insert(binding.original_target);
        }
        if world.get_entity(surface).is_ok() {
            world.entity_mut(surface).remove::<SurfaceBinding>();
            if let Some(mut presentation) = world.get_mut::<WorkbenchPresentation>(surface) {
                *presentation = default();
            }
            if binding.managed_ui_camera {
                if let Some(mut capture) = world.get_mut::<bedtime_ui2::UiCapture>(surface) {
                    *capture = default();
                }
            }
            #[cfg(feature = "native")]
            if let Some(background) = binding.original_background {
                world.entity_mut(surface).insert(background);
            } else {
                world.entity_mut(surface).remove::<WindowBackground>();
            }
        }
    });
}

impl Default for WorkbenchShell {
    fn default() -> Self {
        Self {
            titlebar_ownership: default(),
            panels_visible: true,
            bounds: None,
            minimum_area_size: Vec2::new(100.0, 80.0),
        }
    }
}

/// Owns surface wiring after application startup and before UI setup/drawing.
pub(crate) fn configure(world: &mut World) {
    let surfaces: Vec<_> = world
        .query_filtered::<Entity, With<WorkbenchSurface>>()
        .iter(world)
        .collect();

    assert!(
        surfaces.len() <= 1,
        "Only one WorkbenchSurface is supported"
    );

    let Some(&surface) = surfaces.first() else {
        return;
    };

    assert!(
        world.get::<Window>(surface).is_some(),
        "WorkbenchSurface requires a window"
    );
    let settings = world.get::<WorkbenchSurface>(surface).unwrap();
    assert!(
        [settings.rounding, settings.hit_size, settings.panel_gap]
            .into_iter()
            .chain(settings.padding)
            .all(|value| value.is_finite() && value >= 0.0),
        "Workbench surface dimensions must be finite and nonnegative"
    );
    if let Some(titlebar) = world.get::<Titlebar>(surface) {
        assert!(
            titlebar.height.is_finite() && titlebar.height > 0.0,
            "Titlebar height must be finite and positive"
        );
    }

    let marked: Vec<_> = world
        .query_filtered::<Entity, With<WorkbenchCamera>>()
        .iter(world)
        .collect();
    assert!(marked.len() <= 1, "Only one WorkbenchCamera is supported");
    let camera = *marked
        .first()
        .expect("WorkbenchSurface requires an explicit WorkbenchCamera");
    let initializing = world.get::<SurfaceBinding>(surface).is_none();
    let managed_ui_camera = world
        .get::<UiCamera>(camera)
        .is_none_or(|camera| camera.0 == Entity::PLACEHOLDER);
    if world.get::<UiCamera>(camera).is_none() {
        world
            .entity_mut(camera)
            .insert(UiCamera(Entity::PLACEHOLDER));
    }
    let original_target = world
        .get::<RenderTarget>(camera)
        .expect("WorkbenchCamera requires a camera")
        .clone();
    let quit_shortcuts = initializing.then(|| {
        world
            .get_mut::<bevy_egui::EguiContext>(camera)
            .unwrap()
            .get_mut()
            .options(|options| options.quit_shortcuts.clone())
    });

    let existing: Vec<_> = world
        .query::<(Entity, &UiCamera)>()
        .iter(world)
        .filter_map(|(entity, camera)| (camera.0 == surface).then_some(entity))
        .collect();

    assert!(
        existing.len() <= 1,
        "Only one UI camera can target the workbench surface"
    );

    assert!(
        world.get::<Camera>(camera).is_some(),
        "WorkbenchCamera requires a camera"
    );

    if let Some(&existing) = existing.first() {
        assert_eq!(
            camera, existing,
            "WorkbenchCamera conflicts with the supplied UiCamera"
        );
    }

    if world.get::<UiCamera>(camera).unwrap().0 == Entity::PLACEHOLDER {
        world.entity_mut(camera).insert(UiCamera(surface));
    }

    assert_eq!(
        world.get::<UiCamera>(camera).unwrap().0,
        surface,
        "WorkbenchCamera is associated with another UI surface"
    );
    // Workbench commands and application close policy own Quit. egui's default
    // Cmd/Ctrl+Q handling would consume it before command resolution.
    world
        .get_mut::<bevy_egui::EguiContext>(camera)
        .unwrap()
        .get_mut()
        .options_mut(|options| options.quit_shortcuts.clear());

    match world.get::<RenderTarget>(camera).unwrap() {
        RenderTarget::Window(WindowRef::Entity(target)) => {
            assert_eq!(*target, surface, "Workbench camera targets another window");
        }
        RenderTarget::Window(WindowRef::Primary) => {
            world
                .entity_mut(camera)
                .insert(RenderTarget::Window(WindowRef::Entity(surface)));
        }
        _ => panic!("WorkbenchCamera must target its window"),
    }

    let generated_theme = if world.get::<UsesTheme>(surface).is_none() {
        let theme = world.spawn((themes::dark(), InView(surface))).id();
        world.entity_mut(surface).insert(UsesTheme(theme));
        Some(theme)
    } else {
        None
    };

    if initializing {
        let root = register_view(world, surface, crate::ui::draw_active);
        let binding = SurfaceBinding {
            root,
            camera,
            managed_ui_camera,
            original_target,
            quit_shortcuts: quit_shortcuts.unwrap(),
            generated_theme,
            #[cfg(feature = "native")]
            original_background: world.get::<WindowBackground>(surface).copied(),
        };
        world.entity_mut(surface).insert(binding);
    } else {
        let binding = world.get::<SurfaceBinding>(surface).unwrap();
        assert_eq!(
            binding.camera, camera,
            "Workbench camera changed without detaching its surface"
        );
        let root = binding.root;
        assert!(
            world.get::<bedtime_ui2::UiView>(root).is_some(),
            "Workbench root was removed"
        );
        if let Some(theme) = generated_theme {
            let previous = world
                .get_mut::<SurfaceBinding>(surface)
                .unwrap()
                .generated_theme
                .replace(theme);
            if let Some(previous) = previous {
                if world.get_entity(previous).is_ok() {
                    world.despawn(previous);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removing_and_reattaching_surface_cleans_owned_resources_and_preserves_application_state() {
        let mut app = App::new();
        app.add_plugins(WorkbenchUiPlugin);
        let surface = app
            .world_mut()
            .spawn((
                Window::default(),
                WorkbenchSurface::default(),
                Titlebar {
                    height: 20.0,
                    draw: |_, _, _| {},
                },
            ))
            .id();
        let camera = app.world_mut().spawn((Camera2d, WorkbenchCamera)).id();
        app.update();
        let root = app.world().get::<SurfaceBinding>(surface).unwrap().root;
        let generated = app.world().get::<UsesTheme>(surface).unwrap().0;
        let supplied = app.world_mut().spawn(themes::light()).id();
        app.world_mut()
            .entity_mut(surface)
            .insert(UsesTheme(supplied));
        app.world_mut()
            .entity_mut(surface)
            .remove::<WorkbenchSurface>();
        assert!(app.world().get_entity(root).is_err());
        assert!(app.world().get_entity(generated).is_err());
        assert!(app.world().get::<Window>(surface).is_some());
        assert!(app.world().get::<Titlebar>(surface).is_some());
        assert!(app.world().get::<Camera>(camera).is_some());
        assert!(app.world().get::<UiCamera>(camera).is_none());
        assert!(matches!(
            app.world().get::<RenderTarget>(camera),
            Some(RenderTarget::Window(WindowRef::Primary))
        ));
        assert_eq!(app.world().get::<UsesTheme>(surface).unwrap().0, supplied);
        assert!(app.world().get::<bevy_egui::EguiContext>(camera).is_none());
        app.update();
        app.world_mut()
            .entity_mut(surface)
            .insert(WorkbenchSurface::default());
        app.update();
        assert_ne!(
            app.world().get::<SurfaceBinding>(surface).unwrap().root,
            root
        );
        assert_eq!(app.world().get::<UiCamera>(camera).unwrap().0, surface);
        assert!(
            app.world_mut()
                .get_mut::<bevy_egui::EguiContext>(camera)
                .unwrap()
                .get_mut()
                .options(|options| options.quit_shortcuts.is_empty())
        );
        assert_eq!(app.world().get::<UsesTheme>(surface).unwrap().0, supplied);
        app.world_mut()
            .entity_mut(surface)
            .remove::<WorkbenchSurface>();
        assert!(app.world().get::<bedtime_ui2::Theme>(supplied).is_some());
        assert!(app.world().get::<UiCamera>(camera).is_none());
    }

    #[test]
    fn removing_surface_preserves_supplied_ui_binding_and_unrelated_views() {
        let mut app = App::new();
        app.add_plugins(WorkbenchUiPlugin);
        let theme = app.world_mut().spawn(themes::dark()).id();
        let surface = app
            .world_mut()
            .spawn((
                Window::default(),
                WorkbenchSurface::default(),
                UsesTheme(theme),
            ))
            .id();
        let camera = app
            .world_mut()
            .spawn((Camera2d, WorkbenchCamera, UiCamera(surface)))
            .id();
        let shortcuts = app
            .world_mut()
            .get_mut::<bevy_egui::EguiContext>(camera)
            .unwrap()
            .get_mut()
            .options(|options| options.quit_shortcuts.clone());
        let view = register_view(app.world_mut(), surface, |_, _, _| {});
        app.update();
        let root = app.world().get::<SurfaceBinding>(surface).unwrap().root;
        app.world_mut()
            .entity_mut(surface)
            .remove::<WorkbenchSurface>();
        assert!(app.world().get_entity(root).is_err());
        assert!(app.world().get::<bedtime_ui2::UiView>(view).is_some());
        assert_eq!(app.world().get::<UiCamera>(camera).unwrap().0, surface);
        assert_eq!(app.world().get::<UsesTheme>(surface).unwrap().0, theme);
        assert!(app.world().get::<bedtime_ui2::Theme>(theme).is_some());
        assert_eq!(
            app.world_mut()
                .get_mut::<bevy_egui::EguiContext>(camera)
                .unwrap()
                .get_mut()
                .options(|options| options.quit_shortcuts.clone()),
            shortcuts
        );
        app.update();
    }

    #[test]
    fn startup_components_are_wired_before_ui_setup_and_defaults_retire_with_window() {
        let mut app = App::new();
        app.add_plugins(WorkbenchUiPlugin)
            .add_systems(Startup, |mut commands: Commands| {
                commands.spawn((Window::default(), WorkbenchSurface::default()));
                commands.spawn((Camera2d, WorkbenchCamera));
            })
            .add_systems(
                PreUpdate,
                (|surfaces: Query<Entity, With<WorkbenchSurface>>,
                  themes: Query<&UsesTheme>,
                  cameras: Query<&UiCamera>,
                  views: Query<&bedtime_ui2::OnSurface, With<bedtime_ui2::UiView>>| {
                    for surface in &surfaces {
                        assert!(themes.get(surface).is_ok());
                        assert_eq!(cameras.single().unwrap().0, surface);
                        assert_eq!(views.single().unwrap().0, surface);
                    }
                })
                .in_set(bedtime_ui2::UiSystems::Configure),
            );
        app.update();
        let world = app.world_mut();
        let surface = world
            .query_filtered::<Entity, With<WorkbenchSurface>>()
            .single(world)
            .unwrap();
        let camera = world
            .query_filtered::<Entity, With<UiCamera>>()
            .single(world)
            .unwrap();
        let theme = world.get::<UsesTheme>(surface).unwrap().0;
        let root = world
            .query_filtered::<Entity, With<bedtime_ui2::UiView>>()
            .single(world)
            .unwrap();
        assert_eq!(
            *world.get::<UiBackground>(surface).unwrap(),
            UiBackground::Transparent
        );
        assert_eq!(
            world.get::<WorkbenchSurface>(surface),
            Some(&WorkbenchSurface::default())
        );
        app.update();
        let world = app.world_mut();
        assert_eq!(world.query::<&UiCamera>().iter(world).count(), 1);
        assert_eq!(world.get::<UiCamera>(camera).unwrap().0, surface);
        assert_eq!(world.get::<UsesTheme>(surface).unwrap().0, theme);
        assert_eq!(world.query::<&bedtime_ui2::UiView>().iter(world).count(), 1);
        world.despawn(surface);
        assert!(world.get_entity(camera).is_ok());
        assert!(world.get_entity(theme).is_err());
        assert!(world.get_entity(root).is_err());
        app.update();
    }

    #[test]
    fn supplied_theme_camera_and_shell_configuration_are_preserved() {
        let mut app = App::new();
        app.add_plugins(WorkbenchUiPlugin);
        let world = app.world_mut();
        let theme = world.spawn(themes::light()).id();
        let settings = WorkbenchSurface {
            rounding: 11.0,
            panel_gap: 8.0,
            ..WorkbenchSurface::single_panel()
        };
        let surface = world
            .spawn((
                Window::default(),
                settings,
                UsesTheme(theme),
                UiBackground::Theme,
                WorkbenchShell {
                    minimum_area_size: Vec2::new(120.0, 90.0),
                    ..default()
                },
            ))
            .id();
        // The surface may already exist when application startup supplies its
        // camera. Setup must wait until both have been supplied.
        app.add_systems(Startup, |mut commands: Commands| {
            commands.spawn((
                Camera2d,
                Camera {
                    order: 7,
                    ..default()
                },
                WorkbenchCamera,
            ));
        });
        app.update();
        let world = app.world_mut();
        let (camera, ui_camera) = world.query::<(Entity, &UiCamera)>().single(world).unwrap();
        assert_eq!(ui_camera.0, surface);
        assert_eq!(world.get::<Camera>(camera).unwrap().order, 7);
        assert_eq!(world.get::<UsesTheme>(surface).unwrap().0, theme);
        assert_eq!(
            *world.get::<UiBackground>(surface).unwrap(),
            UiBackground::Theme
        );
        assert_eq!(world.get::<WorkbenchSurface>(surface), Some(&settings));
        assert_eq!(
            world
                .get::<WorkbenchShell>(surface)
                .unwrap()
                .minimum_area_size,
            Vec2::new(120.0, 90.0)
        );
        world.despawn(surface);
        assert!(world.get_entity(camera).is_ok());
        assert!(world.get_entity(theme).is_ok());
    }

    #[test]
    fn a_marked_ui_camera_is_reused() {
        let mut app = App::new();
        app.add_plugins(WorkbenchUiPlugin);
        let world = app.world_mut();
        let surface = world
            .spawn((Window::default(), WorkbenchSurface::default()))
            .id();
        let camera = world
            .spawn((
                Camera2d,
                WorkbenchCamera,
                UiCamera(surface),
                RenderTarget::Window(WindowRef::Entity(surface)),
            ))
            .id();
        assert!(world.get::<bevy_egui::EguiContext>(camera).is_some());
        app.update();
        let world = app.world_mut();
        assert_eq!(
            world
                .query_filtered::<Entity, With<UiCamera>>()
                .single(world)
                .unwrap(),
            camera
        );
        world.despawn(surface);
        assert!(world.get_entity(camera).is_ok());
        assert!(world.get::<UiCamera>(camera).is_none());
    }

    #[test]
    #[should_panic(expected = "WorkbenchSurface requires an explicit WorkbenchCamera")]
    fn a_missing_camera_is_rejected() {
        let mut app = App::new();
        app.add_plugins(WorkbenchUiPlugin);
        app.world_mut()
            .spawn((Window::default(), WorkbenchSurface::default()));
        app.update();
    }

    #[test]
    #[should_panic(expected = "WorkbenchSurface requires an explicit WorkbenchCamera")]
    fn an_unmarked_ui_camera_is_rejected() {
        let mut app = App::new();
        app.add_plugins(WorkbenchUiPlugin);
        let world = app.world_mut();
        let surface = world
            .spawn((Window::default(), WorkbenchSurface::default()))
            .id();
        world.spawn((Camera2d, UiCamera(surface)));
        app.update();
    }

    #[test]
    #[should_panic(expected = "Only one WorkbenchCamera is supported")]
    fn duplicate_marked_cameras_are_rejected() {
        let mut app = App::new();
        app.add_plugins(WorkbenchUiPlugin);
        let world = app.world_mut();
        world.spawn((Window::default(), WorkbenchSurface::default()));
        world.spawn((Camera2d, WorkbenchCamera));
        world.spawn((Camera2d, WorkbenchCamera));
        app.update();
    }

    #[test]
    #[should_panic(expected = "Workbench camera targets another window")]
    fn a_supplied_camera_targeting_another_window_is_rejected() {
        let mut app = App::new();
        app.add_plugins(WorkbenchUiPlugin);
        let world = app.world_mut();
        world.spawn((Window::default(), WorkbenchSurface::default()));
        let other_window = world.spawn(Window::default()).id();
        world.spawn((
            Camera2d,
            WorkbenchCamera,
            RenderTarget::Window(WindowRef::Entity(other_window)),
        ));
        app.update();
    }

    #[test]
    #[should_panic(expected = "Only one WorkbenchSurface")]
    fn duplicate_surfaces_are_rejected() {
        let mut app = App::new();
        app.add_plugins(WorkbenchUiPlugin);
        let world = app.world_mut();
        world.spawn((Window::default(), WorkbenchSurface::default()));
        world.spawn((Window::default(), WorkbenchSurface::default()));
        app.update();
    }
}
