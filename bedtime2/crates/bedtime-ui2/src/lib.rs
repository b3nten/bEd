//! Immediate drawing with ECS surfaces/themes and private control animations.
//! The host installs Bevy's renderer/window and selects its own themes.
mod controls;
pub mod gallery;
mod icon_cache;
pub mod icons;
mod input_map;
mod motion;
mod platform;
mod render;
mod runtime;
pub mod theme;
mod viewport;
mod viewport_textures;

pub use controls::Controls;
pub use dear_imgui_rs as imgui;
pub use icons::{BuiltinIcon, Icon, IconSet, IconSource, UiIconStatus, UiIcons};
pub use motion::MotionPolicy;
pub use render::{UiGraph, UiRenderSystems};
pub use theme::{Theme, themes};
pub use viewport::{UiViewport, viewport_image};

use bevy::{
    ecs::{
        relationship::RelationshipTarget,
        system::{SystemId, SystemInput},
    },
    prelude::*,
};
use dear_imgui_rs::Ui;
use std::cell::RefCell;

/// Opt an existing window into UI rendering. Stage 2 supports one surface/context.
#[derive(Component, Default)]
#[require(UiCapture, MotionPolicy, UiDiagnostics, UiIconStatus)]
pub struct UiSurface;

#[derive(Component, Default, Clone, Copy, Debug)]
pub struct UiCapture {
    pub keyboard: bool,
    pub pointer: bool,
    pub text: bool,
}

/// Per-surface observations, useful for galleries and application diagnostics.
#[derive(Component, Default, Clone, Debug)]
pub struct UiDiagnostics {
    pub frames: u64,
    pub vertices: usize,
    pub indices: usize,
    pub draw_ms: f64,
    pub error: Option<String>,
    pub active_animations: usize,
    pub icon_textures: usize,
    pub icon_rasterizations: u64,
    pub viewport_textures: usize,
}

#[derive(Component)]
#[relationship(relationship_target = ThemeUsers)]
pub struct UsesTheme(pub Entity);
#[derive(Component, Default)]
#[relationship_target(relationship = UsesTheme)]
pub struct ThemeUsers(Vec<Entity>);

/// Ordered ownership of drawing systems by a surface. Despawning a surface
/// retires its views; themes are referenced independently and remain alive.
#[derive(Component)]
#[relationship(relationship_target = SurfaceViews)]
pub struct OnSurface(pub Entity);
#[derive(Component, Default)]
#[relationship_target(relationship = OnSurface, linked_spawn)]
pub struct SurfaceViews(Vec<Entity>);

#[derive(Component)]
#[relationship(relationship_target = ViewParts)]
pub struct InView(pub Entity);
#[derive(Component, Default)]
#[relationship_target(relationship = InView, linked_spawn)]
pub struct ViewParts(Vec<Entity>);

/// Marker on the registered drawing-system entity. Add application state here.
#[derive(Component)]
pub struct UiView;

#[derive(Message, Clone, Debug)]
pub struct UiError(pub String);

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UiSystems {
    Draw,
    PrepareTargets,
}

/// Borrowed system input. It cannot be retained past the active ImGui frame.
pub struct UiFrame<'a> {
    pub view: Entity,
    pub surface: Entity,
    pub ui: &'a Ui,
    pub theme: &'a Theme,
    pub motion: MotionPolicy,
    animation_owner: Entity,
    animations: &'a RefCell<motion::Animations>,
    icons: &'a RefCell<icon_cache::IconCache>,
    viewports: &'a RefCell<viewport_textures::ViewportTextures>,
}
impl SystemInput for UiFrame<'_> {
    type Param<'i> = UiFrame<'i>;
    type Inner<'i> = UiFrame<'i>;
    fn wrap(this: Self::Inner<'_>) -> Self::Param<'_> {
        this
    }
}
impl UiFrame<'_> {
    /// Reborrow a frame for a nested drawing-system entity. The enclosing view
    /// retains the frame/cache lifetime; native widget IDs distinguish controls.
    pub fn for_view(&self, view: Entity) -> UiFrame<'_> {
        UiFrame {
            view,
            surface: self.surface,
            ui: self.ui,
            theme: self.theme,
            motion: self.motion,
            animation_owner: self.animation_owner,
            animations: self.animations,
            icons: self.icons,
            viewports: self.viewports,
        }
    }
    pub fn controls(&self) -> Controls<'_> {
        Controls::new(
            self.ui,
            self.theme,
            self.motion,
            self.animation_owner,
            self.animations,
            self.icons,
            self.viewports,
        )
    }
}

pub struct UiPlugin;
impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        assert!(
            app.get_sub_app(bevy::render::RenderApp).is_some(),
            "Install Bevy's renderer before UiPlugin"
        );
        assert!(
            app.is_plugin_added::<bevy::winit::WinitPlugin>(),
            "Install Bevy's WinitPlugin before UiPlugin"
        );
        icons::install(app);
        app.add_message::<UiError>()
            .init_resource::<bedtime_clipboard2::Clipboard>()
            .insert_non_send(runtime::Runtime::new())
            .configure_sets(
                PostUpdate,
                (UiSystems::Draw, UiSystems::PrepareTargets)
                    .chain()
                    .before(bevy::camera::CameraUpdateSystems),
            )
            .add_systems(
                PostUpdate,
                viewport::reset_visibility.before(UiSystems::Draw),
            )
            .add_systems(PostUpdate, runtime::draw.in_set(UiSystems::Draw))
            .add_systems(
                PostUpdate,
                viewport::resize_targets.in_set(UiSystems::PrepareTargets),
            )
            .add_systems(Last, runtime::shutdown.after(bevy::app::OnAppExitSystems));
        render::install(app);
    }
}

/// Register an ordinary Bevy system, with its state attached to the returned entity.
pub fn register_view<M>(
    world: &mut World,
    surface: Entity,
    draw: impl IntoSystem<UiFrame<'static>, (), M> + 'static,
) -> Entity {
    assert!(
        world.get::<UiSurface>(surface).is_some(),
        "Drawing requires a UiSurface"
    );
    let id: SystemId<UiFrame<'static>> = world.register_system(draw);
    world
        .entity_mut(id.entity())
        .insert((UiView, OnSurface(surface)));
    id.entity()
}

#[cfg(test)]
static IMGUI_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
#[cfg(test)]
mod icon_tests;
#[cfg(test)]
mod tests;
