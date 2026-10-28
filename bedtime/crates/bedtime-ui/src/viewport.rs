use crate::{UiContext, UiSystems};
use bevy::{
    camera::{Projection, RenderTarget},
    prelude::*,
};
use bevy_egui::{EguiTextureHandle, EguiUserTextures};

pub(crate) fn install(app: &mut App) {
    app.configure_sets(
        PostUpdate,
        (UiSystems::SyncViewports, UiSystems::PrepareTargets)
            .chain()
            .before(bevy::camera::CameraUpdateSystems),
    )
    .add_systems(PostUpdate, resize_targets.in_set(UiSystems::PrepareTargets));
}

/// A scene output and its latest UI observations. Attach to the drawing view,
/// then call `UiViewport::draw(ui, world, context, entity)` through [`crate::Component`].
/// Drawing fills the supplied UI allocation and registers the image with egui.
/// Use one output image per independently sized viewport/camera.
#[derive(Component, Clone, Debug)]
pub struct UiViewport {
    pub image: Handle<Image>,
    pub physical_size: UVec2,
    pub rect: Rect,
    pub visible: bool,
    pub hovered: bool,
    /// Optional rounding for an image that meets the surrounding surface edges.
    pub corner_radius: egui::CornerRadius,
}

impl UiViewport {
    pub fn new(image: Handle<Image>) -> Self {
        Self {
            image,
            physical_size: UVec2::ONE,
            rect: Rect::default(),
            visible: false,
            hovered: false,
            corner_radius: egui::CornerRadius::ZERO,
        }
    }

    /// Fill the available allocation and return its ordinary egui interaction.
    /// Callers can handle scene controls without a second input-routing layer.
    pub fn show(ui: &mut egui::Ui, world: &mut World, entity: Entity) -> egui::Response {
        let image = world
            .get::<UiViewport>(entity)
            .expect("Viewport entity requires UiViewport")
            .image
            .id();
        // The component and camera own the image. Upstream caches this mapping
        // and retires weak registrations when the image asset is removed.
        let texture = world
            .resource_mut::<EguiUserTextures>()
            .add_image(EguiTextureHandle::Weak(image));
        let (rect, _) = ui.allocate_exact_size(
            ui.available_size().max(egui::Vec2::splat(1.0)),
            egui::Sense::hover(),
        );
        let response = ui.interact(
            rect,
            ui.id().with(entity.to_bits()),
            egui::Sense::CLICK | egui::Sense::DRAG,
        );
        let mut viewport = world.get_mut::<UiViewport>(entity).unwrap();
        viewport.rect = Rect::from_corners(
            Vec2::new(rect.min.x, rect.min.y),
            Vec2::new(rect.max.x, rect.max.y),
        );
        viewport.visible = ui.is_rect_visible(rect);
        viewport.hovered = response.hovered();
        viewport.physical_size = (viewport.rect.size() * ui.ctx().pixels_per_point())
            .ceil()
            .as_uvec2()
            .max(UVec2::ONE);
        egui::Image::new(egui::load::SizedTexture::new(texture, rect.size()))
            .corner_radius(viewport.corner_radius)
            .paint_at(ui, rect);
        response
    }
}

impl crate::Component for UiViewport {
    type Props<'a> = Entity;

    fn draw(ui: &mut egui::Ui, world: &mut World, _: &UiContext<'_>, entity: Entity) {
        Self::show(ui, world, entity);
    }
}

/// Standard Bevy render target with an sRGB camera view and an encoded-color UI
/// view. This lets the scene use its normal linear lighting and post-processing.
pub fn viewport_image(size: UVec2) -> Image {
    assert!(
        size.min_element() > 0,
        "Viewport dimensions must be nonzero"
    );
    let mut image = Image::new_target_texture(
        size.x,
        size.y,
        bevy::render::render_resource::TextureFormat::Rgba8Unorm,
        Some(bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb),
    );
    image.data = None;
    image.copy_on_resize = false;
    image
}

pub(crate) fn reset_visibility(mut viewports: Query<&mut UiViewport>) {
    for mut viewport in &mut viewports {
        if viewport.visible || viewport.hovered {
            viewport.visible = false;
            viewport.hovered = false;
        }
    }
}

pub(crate) fn resize_targets(
    viewports: Query<&UiViewport>,
    mut images: ResMut<Assets<Image>>,
    mut cameras: Query<(&RenderTarget, &mut Projection)>,
) {
    let mut resized = std::collections::HashSet::new();
    for viewport in &viewports {
        if !viewport.visible {
            continue;
        }
        let Some(image) = images.get(&viewport.image) else {
            continue;
        };
        if image.size() == viewport.physical_size {
            continue;
        }
        let size = viewport.physical_size;
        images
            .get_mut(&viewport.image)
            .unwrap()
            .resize(bevy::render::render_resource::Extent3d {
                width: size.x,
                height: size.y,
                depth_or_array_layers: 1,
            });
        resized.insert(viewport.image.id());
    }
    // Camera updates normally see image events one frame later. Request an
    // immediate projection refresh so shrinking a target never leaves a camera
    // issuing a viewport/scissor larger than its new GPU attachment.
    for (target, mut projection) in &mut cameras {
        if let RenderTarget::Image(target) = target {
            if resized.contains(&target.handle.id()) {
                projection.set_changed();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Component as _;
    use bevy::ecs::system::RunSystemOnce;

    #[test]
    fn returned_response_keeps_dragging_outside_the_viewport_until_release() {
        let mut world = World::new();
        world.init_resource::<Assets<Image>>();
        world.init_resource::<EguiUserTextures>();
        let image = world
            .resource_mut::<Assets<Image>>()
            .add(viewport_image(UVec2::ONE));
        let entity = world.spawn(UiViewport::new(image)).id();
        let ctx = egui::Context::default();
        let start = egui::pos2(40.0, 40.0);
        let end = egui::pos2(200.0, 70.0);
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: default(),
        };
        let mut observations = Vec::new();
        for (frame, events) in [
            vec![egui::Event::PointerMoved(start)],
            vec![button(start, true)],
            vec![egui::Event::PointerMoved(end)],
            vec![button(end, false)],
            vec![egui::Event::PointerMoved(start)],
        ]
        .into_iter()
        .enumerate()
        {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(400.0, 300.0),
                    )),
                    time: Some(frame as f64 / 60.0),
                    events,
                    ..default()
                },
                |ui| {
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(
                        egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(100.0, 100.0)),
                    ));
                    let response = UiViewport::show(&mut child, &mut world, entity);
                    observations.push((
                        response.dragged(),
                        response.drag_stopped(),
                        response.drag_delta(),
                    ));
                },
            );
            output.textures_delta.clear();
        }
        assert!(observations[2].0, "The drag continues outside the image");
        assert!(observations[2].2.x > 0.0);
        assert!(observations[3].1, "Releasing outside ends the drag");
        assert!(
            !observations[4].0,
            "Returning without a press cannot resume it"
        );
    }

    #[test]
    fn drawing_registers_new_images_immediately_and_keeps_their_identity_when_resized() {
        let mut world = World::new();
        world.init_resource::<Assets<Image>>();
        world.init_resource::<EguiUserTextures>();
        let ctx = egui::Context::default();
        let theme = crate::Theme::dark();
        let icons = std::sync::Mutex::new(crate::icon_cache::IconCache::default());
        let mut entity = None;
        let mut texture = None;
        for (size, dpi) in [
            (egui::vec2(160.0, 90.0), 1.0),
            (egui::vec2(100.0, 60.0), 2.0),
        ] {
            ctx.set_pixels_per_point(dpi);
            let bounds = egui::Rect::from_min_size(egui::pos2(20.0, 30.0), size);
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(500.0, 400.0),
                    )),
                    events: vec![egui::Event::PointerMoved(bounds.center())],
                    ..default()
                },
                |ui| {
                    // A parent can create and draw its child in the same UI pass,
                    // without a runtime preregistration pass or a UiFrame.
                    let entity = *entity.get_or_insert_with(|| {
                        let image = world
                            .resource_mut::<Assets<Image>>()
                            .add(viewport_image(UVec2::ONE));
                        world.spawn(UiViewport::new(image)).id()
                    });
                    let context = UiContext {
                        view: entity,
                        surface: Entity::PLACEHOLDER,
                        theme: &theme,
                        metrics: &crate::Metrics::default(),
                        motion: default(),
                        icons: &icons,
                    };
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(bounds));
                    UiViewport::draw(&mut child, &mut world, &context, entity);
                },
            );
            output.textures_delta.clear();
            let viewport = world.get::<UiViewport>(entity.unwrap()).unwrap();
            assert!(viewport.visible);
            assert_eq!(viewport.rect.min, Vec2::new(bounds.min.x, bounds.min.y));
            assert_eq!(
                viewport.physical_size,
                UVec2::new((size.x * dpi) as u32, (size.y * dpi) as u32)
            );
            let image = viewport.image.id();
            let registered = world
                .resource::<EguiUserTextures>()
                .image_id(image)
                .unwrap();
            assert_eq!(*texture.get_or_insert(registered), registered);
            assert!(ctx.tessellate(output.shapes, output.pixels_per_point).iter().any(|primitive| {
                matches!(&primitive.primitive, egui::epaint::Primitive::Mesh(mesh) if mesh.texture_id == registered && !mesh.indices.is_empty())
            }), "The image is painted on the first draw");
            world.run_system_once(resize_targets).unwrap();
            assert_eq!(
                world.resource::<Assets<Image>>().get(image).unwrap().size(),
                UVec2::new((size.x * dpi) as u32, (size.y * dpi) as u32)
            );
            assert_eq!(
                world.resource::<EguiUserTextures>().image_id(image),
                Some(registered)
            );
        }
    }

    #[test]
    fn removing_a_viewport_releases_its_image_without_unregistering_shared_images() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()))
            .init_asset::<Image>()
            .init_resource::<EguiUserTextures>()
            .init_resource::<bevy_egui::EguiManagedTextures>()
            .add_systems(Last, bevy_egui::free_egui_textures_system);
        let image = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .add(viewport_image(UVec2::ONE));
        let id = image.id();
        let viewport = app.world_mut().spawn(UiViewport::new(image.clone())).id();
        let ctx = egui::Context::default();
        let theme = crate::Theme::dark();
        let icons = std::sync::Mutex::new(crate::icon_cache::IconCache::default());
        let context = UiContext {
            view: viewport,
            surface: Entity::PLACEHOLDER,
            theme: &theme,
            metrics: &crate::Metrics::default(),
            motion: default(),
            icons: &icons,
        };
        let mut output = ctx.run_ui(default(), |ui| {
            UiViewport::draw(ui, app.world_mut(), &context, viewport);
        });
        output.textures_delta.clear();
        app.world_mut().despawn(viewport);
        for _ in 0..3 {
            app.update();
        }
        assert!(app.world().resource::<Assets<Image>>().contains(id));
        assert!(
            app.world()
                .resource::<EguiUserTextures>()
                .image_id(id)
                .is_some(),
            "An image still owned by the application keeps its registration"
        );
        drop(image);
        for _ in 0..3 {
            app.update();
        }
        assert!(!app.world().resource::<Assets<Image>>().contains(id));
        assert_eq!(
            app.world().resource::<EguiUserTextures>().image_id(id),
            None,
            "Drawing must not keep the image or registration alive indefinitely"
        );
    }

    #[test]
    fn syncing_a_camera_to_a_resized_target_updates_its_projection_in_the_same_frame() {
        use bevy::{
            render::{camera::camera_system, texture::ManualTextureViews},
            window::{WindowCreated, WindowResized, WindowScaleFactorChanged},
        };
        let mut app = App::new();
        app.init_resource::<Assets<Image>>()
            .init_resource::<ManualTextureViews>()
            .add_message::<AssetEvent<Image>>()
            .add_message::<WindowCreated>()
            .add_message::<WindowResized>()
            .add_message::<WindowScaleFactorChanged>()
            .add_systems(
                PostUpdate,
                camera_system.in_set(bevy::camera::CameraUpdateSystems),
            );
        install(&mut app);
        let image = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .add(viewport_image(UVec2::new(100, 80)));
        let camera = app
            .world_mut()
            .spawn((Camera3d::default(), RenderTarget::Image(image.into())))
            .id();
        app.update();
        assert_eq!(
            app.world()
                .get::<Camera>(camera)
                .unwrap()
                .physical_target_size(),
            Some(UVec2::new(100, 80))
        );

        let next = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .add(viewport_image(UVec2::new(300, 180)));
        let mut viewport = UiViewport::new(next.clone());
        viewport.visible = true;
        viewport.physical_size = UVec2::new(60, 40);
        let view = app.world_mut().spawn(viewport).id();
        app.add_systems(
            PostUpdate,
            (move |mut targets: Query<&mut RenderTarget>| {
                *targets.get_mut(camera).unwrap() = RenderTarget::Image(next.clone().into());
            })
            .in_set(UiSystems::SyncViewports),
        );
        // No asset-event plugin: this must refresh the camera this frame,
        // without waiting for next frame's image modification notification.
        for size in [UVec2::new(60, 40), UVec2::new(30, 20)] {
            app.world_mut()
                .get_mut::<UiViewport>(view)
                .unwrap()
                .physical_size = size;
            app.update();
            assert_eq!(
                app.world()
                    .get::<Camera>(camera)
                    .unwrap()
                    .physical_target_size(),
                Some(size)
            );
            let Projection::Perspective(projection) =
                app.world().get::<Projection>(camera).unwrap()
            else {
                panic!("3D camera uses a perspective projection");
            };
            assert_eq!(projection.aspect_ratio, size.x as f32 / size.y as f32);
        }
    }

    #[test]
    fn target_resize_refreshes_only_its_camera_and_hidden_views_keep_their_size() {
        let mut world = World::new();
        world.init_resource::<Assets<Image>>();
        let image = world
            .resource_mut::<Assets<Image>>()
            .add(viewport_image(UVec2::new(100, 80)));
        let camera = world
            .spawn((
                Camera3d::default(),
                RenderTarget::Image(image.clone().into()),
            ))
            .id();
        let other = world.spawn(Camera3d::default()).id();
        let mut viewport = UiViewport::new(image.clone());
        viewport.visible = true;
        viewport.physical_size = UVec2::new(300, 170);
        let view = world.spawn(viewport).id();
        world.clear_trackers();
        world.run_system_once(resize_targets).unwrap();
        assert_eq!(
            world
                .resource::<Assets<Image>>()
                .get(&image)
                .unwrap()
                .size(),
            UVec2::new(300, 170)
        );
        assert!(
            world
                .entity(camera)
                .get_ref::<Projection>()
                .unwrap()
                .is_changed()
        );
        assert!(
            !world
                .entity(other)
                .get_ref::<Projection>()
                .unwrap()
                .is_changed()
        );
        let source = world.resource::<Assets<Image>>().get(&image).unwrap();
        assert_eq!(
            source.texture_descriptor.format,
            bevy::render::render_resource::TextureFormat::Rgba8Unorm
        );
        assert_eq!(
            source.texture_view_descriptor.as_ref().unwrap().format,
            Some(bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb)
        );
        assert!(source.data.is_none());
        assert!(!source.copy_on_resize);
        world.get_mut::<UiViewport>(view).unwrap().physical_size = UVec2::ONE;
        world.run_system_once(reset_visibility).unwrap();
        world.clear_trackers();
        world.run_system_once(resize_targets).unwrap();
        assert_eq!(
            world
                .resource::<Assets<Image>>()
                .get(&image)
                .unwrap()
                .size(),
            UVec2::new(300, 170)
        );
        assert!(
            !world
                .entity(camera)
                .get_ref::<Projection>()
                .unwrap()
                .is_changed()
        );
    }
}
