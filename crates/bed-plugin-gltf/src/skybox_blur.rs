//! Background-only blur using Bevy's already filtered reflection cubemap.
//!
//! A separate image handle aliases the same GPU texture with an independent
//! sampler. Lighting keeps its original sampler, and changing blur requires no
//! texture upload or additional filtering pass.
use bevy::{
    asset::RenderAssetUsages,
    light::{EnvironmentMapLight, GeneratedEnvironmentMapLight},
    pbr::generate::generate_environment_map_light,
    prelude::*,
    render::{
        Render, RenderApp, RenderSystems,
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        render_asset::RenderAssets,
        render_resource::{FilterMode, MipmapFilterMode, SamplerDescriptor, TextureId},
        renderer::RenderDevice,
        texture::GpuImage,
    },
};

#[derive(Resource, Clone, ExtractResource)]
struct SkyboxBlur {
    image: Handle<Image>,
    source: Option<Handle<Image>>,
    amount: f32,
}

/// Install after Bevy's default plugins and return the background image handle.
pub(super) fn install(app: &mut App) -> Handle<Image> {
    // This asset reserves a stable handle. Its GPU representation is supplied
    // below, so it must never be uploaded by Bevy's ordinary image preparation.
    let image = app.world_mut().resource_mut::<Assets<Image>>().add(Image {
        asset_usage: RenderAssetUsages::MAIN_WORLD,
        ..default()
    });
    app.insert_resource(SkyboxBlur {
        image: image.clone(),
        source: None,
        amount: 0.0,
    })
    .add_plugins(ExtractResourcePlugin::<SkyboxBlur>::default())
    .add_systems(Update, update_source.after(generate_environment_map_light));
    app.sub_app_mut(RenderApp).add_systems(
        Render,
        prepare_background.in_set(RenderSystems::PrepareResources),
    );
    image
}

/// Set background blur from sharp (0) to the broadest filtered mip (1).
pub(super) fn configure(app: &mut App, amount: f32) {
    let amount = if amount.is_finite() {
        amount.clamp(0.0, 1.0)
    } else {
        0.0
    };
    app.world_mut().resource_mut::<SkyboxBlur>().amount = amount;
}

fn update_source(
    lights: Query<&EnvironmentMapLight, With<GeneratedEnvironmentMapLight>>,
    mut blur: ResMut<SkyboxBlur>,
) {
    let source = lights.iter().next().map(|light| light.specular_map.clone());
    if blur.source != source {
        blur.source = source;
    }
}

fn prepare_background(
    blur: Res<SkyboxBlur>,
    mut images: ResMut<RenderAssets<GpuImage>>,
    device: Res<RenderDevice>,
    mut previous: Local<Option<(TextureId, u32)>>,
) {
    let source = blur.source.as_ref().and_then(|source| images.get(source));
    if blur.amount == 0.0 || source.is_none() {
        images.remove(&blur.image);
        *previous = None;
        return;
    }
    let source = source.unwrap();
    // Size changes replace the generated reflection texture. Derive the range
    // from the current GPU texture, never from a previous environment's mips.
    let lod = blur.amount * source.texture_descriptor.mip_level_count.saturating_sub(1) as f32;
    let key = (source.texture.id(), lod.to_bits());
    if *previous == Some(key) && images.get(&blur.image).is_some() {
        return;
    }
    let mut background = source.clone();
    background.sampler = device.create_sampler(&SamplerDescriptor {
        label: Some("model skybox blur"),
        mag_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        mipmap_filter: MipmapFilterMode::Linear,
        lod_min_clamp: lod,
        lod_max_clamp: lod,
        ..default()
    });
    images.insert(&blur.image, background);
    *previous = Some(key);
}
