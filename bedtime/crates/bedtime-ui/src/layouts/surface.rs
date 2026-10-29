//! Default shell wiring, with explicit entities remaining the source of truth.
use super::*;
#[cfg(test)]
use crate::Theme;
use crate::{UiBackground, UiCamera, UiSurface, register_view};
use bevy::{camera::RenderTarget, prelude::*, window::WindowRef};

/// Layouts on a window, rendered by an explicit LayoutCamera.
/// Supply a theme or SurfaceShell alongside this to override their defaults.
/// Wiring runs in PreUpdate before crate::UiSystems::Configure.
/// User permissions do not restrict direct application layout operations.
#[derive(Component, Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[component(on_remove = teardown)]
#[require(UiSurface, SurfaceShell, UiBackground::Transparent)]
pub struct LayoutSurface {
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
    /// Whether areas paint the theme's panel background behind their panel.
    pub area_background: AreaBackground,
    /// Enable corner handles for splitting, joining and swapping areas.
    /// Disabling them leaves divider resizing and tab dragging available.
    pub drag_corners: bool,
    pub layout_editing: bool,
    pub panel_creation: bool,
    pub panel_closure: bool,
}

impl Default for LayoutSurface {
    fn default() -> Self {
        Self {
            rounding: 7.0,
            padding: [14.0, 12.0],
            panel_gap: 5.0,
            hit_size: 13.0,
            debug: false,
            single_area_edge_to_edge: true,
            tab_bar: TabBarVisibility::Always,
            area_background: AreaBackground::Always,
            drag_corners: true,
            layout_editing: true,
            panel_creation: true,
            panel_closure: true,
        }
    }
}

impl LayoutSurface {
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
    /// Show an area's tab bar while it has more than one tab.
    MultipleTabs,
    /// Show tab bars while the layout has more than one area.
    MultipleAreas,
    /// Either: hidden only for a layout of one area with one tab, which is
    /// then just that panel. Tabs come back as soon as there's something to
    /// switch between or move between areas.
    MultipleTabsOrAreas,
    Hidden,
}

/// When areas paint a background. Backgrounds separate areas from each other;
/// with one area, leaving it out lets the panel draw straight onto the window
/// (a terminal under a transparent titlebar, say).
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AreaBackground {
    #[default]
    Always,
    /// Only while the layout has more than one area.
    MultipleAreas,
    Never,
}

/// The body of a layout surface's window, under its chrome: by default the
/// active layout (`view::show`). An app draws its own body to put things
/// around the panels (a status bar, say), calling `view::show` for them.
#[derive(Component, Clone, Copy)]
pub struct SurfaceBody(pub crate::UiDraw);

/// Required application-owned camera. UiCamera (and its EguiContext) is supplied
/// immediately; surface configuration resolves its initially unbound target.
#[derive(Component)]
#[require(UiCamera(Entity::PLACEHOLDER))]
pub struct LayoutCamera;

/// Only the resources and camera changes installed by workbench setup.
#[derive(Component, Clone)]
struct SurfaceBinding {
    root: Entity,
    camera: Entity,
    managed_ui_camera: bool,
    original_target: RenderTarget,
    quit_shortcuts: Vec<crate::egui::KeyboardShortcut>,
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
            if let Some(mut presentation) = world.get_mut::<SurfacePresentation>(surface) {
                *presentation = default();
            }
            if binding.managed_ui_camera {
                if let Some(mut capture) = world.get_mut::<crate::UiCapture>(surface) {
                    *capture = default();
                }
            }
            if let Some(background) = binding.original_background {
                world.entity_mut(surface).insert(background);
            } else {
                world.entity_mut(surface).remove::<WindowBackground>();
            }
        }
    });
}

impl Default for SurfaceShell {
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
        .query_filtered::<Entity, With<LayoutSurface>>()
        .iter(world)
        .collect();

    assert!(surfaces.len() <= 1, "Only one LayoutSurface is supported");

    let Some(&surface) = surfaces.first() else {
        return;
    };

    assert!(
        world.get::<Window>(surface).is_some(),
        "LayoutSurface requires a window"
    );
    let settings = world.get::<LayoutSurface>(surface).unwrap();
    assert!(
        [settings.rounding, settings.hit_size, settings.panel_gap]
            .into_iter()
            .chain(settings.padding)
            .all(|value| value.is_finite() && value >= 0.0),
        "Layout surface dimensions must be finite and nonnegative"
    );
    if let Some(titlebar) = world.get::<Titlebar>(surface) {
        assert!(
            titlebar.height.is_finite() && titlebar.height > 0.0,
            "Titlebar height must be finite and positive"
        );
    }

    let marked: Vec<_> = world
        .query_filtered::<Entity, With<LayoutCamera>>()
        .iter(world)
        .collect();
    assert!(marked.len() <= 1, "Only one LayoutCamera is supported");
    let camera = *marked
        .first()
        .expect("LayoutSurface requires an explicit LayoutCamera");
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
        .expect("LayoutCamera requires a camera")
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
        "LayoutCamera requires a camera"
    );

    if let Some(&existing) = existing.first() {
        assert_eq!(
            camera, existing,
            "LayoutCamera conflicts with the supplied UiCamera"
        );
    }

    if world.get::<UiCamera>(camera).unwrap().0 == Entity::PLACEHOLDER {
        world.entity_mut(camera).insert(UiCamera(surface));
    }

    assert_eq!(
        world.get::<UiCamera>(camera).unwrap().0,
        surface,
        "LayoutCamera is associated with another UI surface"
    );
    // The app's actions and close policy own Quit. egui's default
    // Cmd/Ctrl+Q handling would consume it before command resolution.
    world
        .get_mut::<bevy_egui::EguiContext>(camera)
        .unwrap()
        .get_mut()
        .options_mut(|options| options.quit_shortcuts.clear());

    match world.get::<RenderTarget>(camera).unwrap() {
        RenderTarget::Window(WindowRef::Entity(target)) => {
            assert_eq!(*target, surface, "Layout camera targets another window");
        }
        RenderTarget::Window(WindowRef::Primary) => {
            world
                .entity_mut(camera)
                .insert(RenderTarget::Window(WindowRef::Entity(surface)));
        }
        _ => panic!("LayoutCamera must target its window"),
    }

    if initializing {
        let root = register_view(world, surface, super::dialogs::draw_root);
        let binding = SurfaceBinding {
            root,
            camera,
            managed_ui_camera,
            original_target,
            quit_shortcuts: quit_shortcuts.unwrap(),
            original_background: world.get::<WindowBackground>(surface).copied(),
        };
        world.entity_mut(surface).insert(binding);
    } else {
        let binding = world.get::<SurfaceBinding>(surface).unwrap();
        assert_eq!(
            binding.camera, camera,
            "Layout camera changed without detaching its surface"
        );
        let root = binding.root;
        assert!(
            world.get::<crate::UiView>(root).is_some(),
            "The layout surface root was removed"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removing_and_reattaching_surface_cleans_owned_resources_and_preserves_application_state() {
        let mut app = App::new();
        app.add_plugins(super::LayoutsPlugin);
        let surface = app
            .world_mut()
            .spawn((
                Window::default(),
                LayoutSurface::default(),
                Titlebar {
                    height: 20.0,
                    draw: |_, _, _| {},
                },
            ))
            .id();
        let camera = app.world_mut().spawn((Camera2d, LayoutCamera)).id();
        app.update();
        let root = app.world().get::<SurfaceBinding>(surface).unwrap().root;
        // The surface's theme is the application's; detaching keeps it.
        app.world_mut().entity_mut(surface).insert(Theme::light());
        app.world_mut()
            .entity_mut(surface)
            .remove::<LayoutSurface>();
        assert!(app.world().get_entity(root).is_err());
        assert!(app.world().get::<Window>(surface).is_some());
        assert!(app.world().get::<Titlebar>(surface).is_some());
        assert!(app.world().get::<Camera>(camera).is_some());
        assert!(app.world().get::<UiCamera>(camera).is_none());
        assert!(matches!(
            app.world().get::<RenderTarget>(camera),
            Some(RenderTarget::Window(WindowRef::Primary))
        ));
        assert_eq!(app.world().get::<Theme>(surface), Some(&Theme::light()));
        assert!(app.world().get::<bevy_egui::EguiContext>(camera).is_none());
        app.update();
        app.world_mut()
            .entity_mut(surface)
            .insert(LayoutSurface::default());
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
        assert_eq!(app.world().get::<Theme>(surface), Some(&Theme::light()));
        app.world_mut()
            .entity_mut(surface)
            .remove::<LayoutSurface>();
        assert!(app.world().get::<UiCamera>(camera).is_none());
    }

    #[test]
    fn removing_surface_preserves_supplied_ui_binding_and_unrelated_views() {
        let mut app = App::new();
        app.add_plugins(super::LayoutsPlugin);
        let surface = app
            .world_mut()
            .spawn((Window::default(), LayoutSurface::default(), Theme::light()))
            .id();
        let camera = app
            .world_mut()
            .spawn((Camera2d, LayoutCamera, UiCamera(surface)))
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
            .remove::<LayoutSurface>();
        assert!(app.world().get_entity(root).is_err());
        assert!(app.world().get::<crate::UiView>(view).is_some());
        assert_eq!(app.world().get::<UiCamera>(camera).unwrap().0, surface);
        assert_eq!(app.world().get::<Theme>(surface), Some(&Theme::light()));
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
        app.add_plugins(super::LayoutsPlugin)
            .add_systems(Startup, |mut commands: Commands| {
                commands.spawn((Window::default(), LayoutSurface::default()));
                commands.spawn((Camera2d, LayoutCamera));
            })
            .add_systems(
                PreUpdate,
                (|surfaces: Query<Entity, With<LayoutSurface>>,
                  themes: Query<&Theme>,
                  cameras: Query<&UiCamera>,
                  views: Query<&crate::InSurface, With<crate::UiView>>| {
                    for surface in &surfaces {
                        assert!(themes.get(surface).is_ok());
                        assert_eq!(cameras.single().unwrap().0, surface);
                        assert_eq!(views.single().unwrap().0, surface);
                    }
                })
                .in_set(crate::UiSystems::Configure),
            );
        app.update();
        let world = app.world_mut();
        let surface = world
            .query_filtered::<Entity, With<LayoutSurface>>()
            .single(world)
            .unwrap();
        let camera = world
            .query_filtered::<Entity, With<UiCamera>>()
            .single(world)
            .unwrap();
        let root = world
            .query_filtered::<Entity, With<crate::UiView>>()
            .single(world)
            .unwrap();
        assert_eq!(
            *world.get::<UiBackground>(surface).unwrap(),
            UiBackground::Transparent
        );
        assert_eq!(
            world.get::<LayoutSurface>(surface),
            Some(&LayoutSurface::default())
        );
        app.update();
        let world = app.world_mut();
        assert_eq!(world.query::<&UiCamera>().iter(world).count(), 1);
        assert_eq!(world.get::<UiCamera>(camera).unwrap().0, surface);
        assert_eq!(world.get::<Theme>(surface), Some(&Theme::dark()));
        assert_eq!(world.query::<&crate::UiView>().iter(world).count(), 1);
        world.despawn(surface);
        assert!(world.get_entity(camera).is_ok());
        assert!(world.get_entity(root).is_err());
        app.update();
    }

    #[test]
    fn supplied_theme_camera_and_shell_configuration_are_preserved() {
        let mut app = App::new();
        app.add_plugins(super::LayoutsPlugin);
        let world = app.world_mut();
        let settings = LayoutSurface {
            rounding: 11.0,
            panel_gap: 8.0,
            ..LayoutSurface::single_panel()
        };
        let surface = world
            .spawn((
                Window::default(),
                settings,
                Theme::light(),
                UiBackground::Theme,
                SurfaceShell {
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
                LayoutCamera,
            ));
        });
        app.update();
        let world = app.world_mut();
        let (camera, ui_camera) = world.query::<(Entity, &UiCamera)>().single(world).unwrap();
        assert_eq!(ui_camera.0, surface);
        assert_eq!(world.get::<Camera>(camera).unwrap().order, 7);
        assert_eq!(world.get::<Theme>(surface), Some(&Theme::light()));
        assert_eq!(
            *world.get::<UiBackground>(surface).unwrap(),
            UiBackground::Theme
        );
        assert_eq!(world.get::<LayoutSurface>(surface), Some(&settings));
        assert_eq!(
            world
                .get::<SurfaceShell>(surface)
                .unwrap()
                .minimum_area_size,
            Vec2::new(120.0, 90.0)
        );
        world.despawn(surface);
        assert!(world.get_entity(camera).is_ok());
    }

    #[test]
    fn a_marked_ui_camera_is_reused() {
        let mut app = App::new();
        app.add_plugins(super::LayoutsPlugin);
        let world = app.world_mut();
        let surface = world
            .spawn((Window::default(), LayoutSurface::default()))
            .id();
        let camera = world
            .spawn((
                Camera2d,
                LayoutCamera,
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
    #[should_panic(expected = "LayoutSurface requires an explicit LayoutCamera")]
    fn a_missing_camera_is_rejected() {
        let mut app = App::new();
        app.add_plugins(super::LayoutsPlugin);
        app.world_mut()
            .spawn((Window::default(), LayoutSurface::default()));
        app.update();
    }

    #[test]
    #[should_panic(expected = "LayoutSurface requires an explicit LayoutCamera")]
    fn an_unmarked_ui_camera_is_rejected() {
        let mut app = App::new();
        app.add_plugins(super::LayoutsPlugin);
        let world = app.world_mut();
        let surface = world
            .spawn((Window::default(), LayoutSurface::default()))
            .id();
        world.spawn((Camera2d, UiCamera(surface)));
        app.update();
    }

    #[test]
    #[should_panic(expected = "Only one LayoutCamera is supported")]
    fn duplicate_marked_cameras_are_rejected() {
        let mut app = App::new();
        app.add_plugins(super::LayoutsPlugin);
        let world = app.world_mut();
        world.spawn((Window::default(), LayoutSurface::default()));
        world.spawn((Camera2d, LayoutCamera));
        world.spawn((Camera2d, LayoutCamera));
        app.update();
    }

    #[test]
    #[should_panic(expected = "Layout camera targets another window")]
    fn a_supplied_camera_targeting_another_window_is_rejected() {
        let mut app = App::new();
        app.add_plugins(super::LayoutsPlugin);
        let world = app.world_mut();
        world.spawn((Window::default(), LayoutSurface::default()));
        let other_window = world.spawn(Window::default()).id();
        world.spawn((
            Camera2d,
            LayoutCamera,
            RenderTarget::Window(WindowRef::Entity(other_window)),
        ));
        app.update();
    }

    #[test]
    #[should_panic(expected = "Only one LayoutSurface")]
    fn duplicate_surfaces_are_rejected() {
        let mut app = App::new();
        app.add_plugins(super::LayoutsPlugin);
        let world = app.world_mut();
        world.spawn((Window::default(), LayoutSurface::default()));
        world.spawn((Window::default(), LayoutSurface::default()));
        app.update();
    }
}
