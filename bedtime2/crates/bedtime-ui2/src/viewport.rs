//! Bevy cameras own scene rendering. UI owns only the image placement and size request.
use bevy::{
    camera::{Projection, RenderTarget},
    prelude::*,
};

/// A scene output and its latest UI observations. Attach to the drawing view.
/// Use one output image per independently sized viewport/camera.
#[derive(Component, Clone, Debug)]
pub struct UiViewport {
    pub image: Handle<Image>,
    pub physical_size: UVec2,
    pub rect: Rect,
    pub visible: bool,
    pub hovered: bool,
}
impl UiViewport {
    pub fn new(image: Handle<Image>) -> Self {
        Self {
            image,
            physical_size: UVec2::ONE,
            rect: Rect::default(),
            visible: false,
            hovered: false,
        }
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
        wgpu::TextureFormat::Rgba8Unorm,
        Some(wgpu::TextureFormat::Rgba8UnormSrgb),
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
            .resize(wgpu::Extent3d {
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
    use bevy::ecs::system::RunSystemOnce;

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
            wgpu::TextureFormat::Rgba8Unorm
        );
        assert_eq!(
            source.texture_view_descriptor.as_ref().unwrap().format,
            Some(wgpu::TextureFormat::Rgba8UnormSrgb)
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
