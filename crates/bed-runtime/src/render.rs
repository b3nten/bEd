//! Bevy's windowless renderer, using the native host's GPU resources.
//! The host owns acquisition and presentation; plugins own cameras and targets.
use bevy::{
    app::TaskPoolPlugin,
    diagnostic::FrameCountPlugin,
    input::InputPlugin,
    prelude::*,
    render::{RenderPlugin, settings::RenderResources},
    time::TimePlugin,
    window::{ExitCondition, WindowPlugin},
};
use std::collections::{BTreeMap, BTreeSet};

/// Install before finalizing the app, after modules register their asset sources.
pub struct BedRenderPlugin {
    pub resources: RenderResources,
}

impl Plugin for BedRenderPlugin {
    fn build(&self, app: &mut App) {
        // Choose the windowless plugin set explicitly. DefaultPlugins changes
        // with Cargo feature unification: linking the desktop alongside an
        // embedding host must never install Winit or replace its runner.
        if !app.is_plugin_added::<TaskPoolPlugin>() {
            app.add_plugins(TaskPoolPlugin::default());
        }
        if !app.is_plugin_added::<TimePlugin>() {
            app.add_plugins(TimePlugin);
        }
        if !app.is_plugin_added::<InputPlugin>() {
            app.add_plugins(InputPlugin);
        }
        if !app.is_plugin_added::<FrameCountPlugin>() {
            app.add_plugins(FrameCountPlugin);
        }
        app.add_plugins((
            bevy::transform::TransformPlugin,
            bevy::diagnostic::DiagnosticsPlugin,
            WindowPlugin {
                primary_window: None,
                exit_condition: ExitCondition::DontExit,
                ..default()
            },
            bevy::asset::AssetPlugin::default(),
        ));
        #[cfg(feature = "scene")]
        app.add_plugins(bevy::world_serialization::WorldSerializationPlugin);
        app.add_plugins((
            RenderPlugin {
                render_creation: self.resources.clone().into(),
                synchronous_pipeline_compilation: true,
                ..default()
            },
            bevy::image::ImagePlugin::default(),
            bevy::mesh::MeshPlugin,
            bevy::camera::CameraPlugin,
        ));
        #[cfg(feature = "scene")]
        app.add_plugins(bevy::light::LightPlugin);
        app.add_plugins((
            bevy::core_pipeline::CorePipelinePlugin,
            bevy::anti_alias::AntiAliasPlugin,
        ));
        #[cfg(feature = "scene")]
        app.add_plugins((
            bevy::gltf::GltfPlugin::default(),
            bevy::pbr::PbrPlugin::default(),
        ));
        app.add_plugins(BedPreviewPlugin);
    }
}

/// Preview ownership and visibility in either a headful or headless render app.
/// It does not install a renderer or choose a window/application runner.
pub struct BedPreviewPlugin;

impl Plugin for BedPreviewPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PreviewPipelineStatus>();
        app.sub_app_mut(bevy::render::RenderApp)
            .init_resource::<PreviewGpuState>()
            .add_systems(
                bevy::render::ExtractSchedule,
                extract_preview_gpu_state.before(
                    bevy::extract::extract_resource::extract_resource::<
                        bevy::render::texture::ManualTextureViews,
                        bevy::render::RenderApp,
                        (),
                    >,
                ),
            )
            .add_systems(bevy::render::ExtractSchedule, publish_pipeline_status)
            .add_systems(bevy::render::RenderStartup, clear_preview_gpu_targets);
        #[cfg(feature = "scene")]
        app.sub_app_mut(bevy::render::RenderApp)
            .add_systems(
                bevy::render::RenderStartup,
                clear_scene_gpu_state.after(clear_preview_gpu_targets),
            )
            .add_systems(
                bevy::render::Render,
                rebuild_recovered_point_light_shadows
                    .in_set(bevy::render::RenderSystems::CreateViews)
                    .before(bevy::pbr::prepare_lights),
            );
        app.init_resource::<PreviewLayers>().add_systems(
            PostUpdate,
            inherit_preview_layers
                .before(bevy::camera::visibility::VisibilitySystems::CheckVisibility),
        );
    }
}

#[derive(Resource, Default)]
struct PreviewGpuState {
    initialized: bool,
    invalidate_main: bool,
    targets: BTreeSet<bevy::camera::ManualTextureViewHandle>,
}

// RenderStartup runs outside extraction and cannot access MainWorld. Publish
// owned handle facts here, and invalidate the main world's old attachments
// before Bevy copies manual targets and extracts cameras on the recovered frame.
fn extract_preview_gpu_state(world: &mut World) {
    use bevy::{camera::ManualTextureViewHandle, render::texture::ManualTextureViews};
    let invalidate = std::mem::take(&mut world.resource_mut::<PreviewGpuState>().invalidate_main);
    let targets = {
        let mut main = world.resource_mut::<bevy::render::MainWorld>();
        let targets = main
            .resource::<PreviewLayers>()
            .assigned
            .values()
            .map(|layer| ManualTextureViewHandle(u32::try_from(*layer).expect("valid target")))
            .collect::<BTreeSet<_>>();
        if invalidate {
            begin_preview_frame(&mut main);
            // Bevy updates target metadata for inactive cameras too. Detach
            // expired targets until the producer registers its new attachment.
            let cameras = main
                .query_filtered::<(Entity, &bevy::camera::RenderTarget), With<Camera>>()
                .iter(&main)
                .filter_map(|(entity, target)| match target {
                    bevy::camera::RenderTarget::TextureView(target) if targets.contains(target) => {
                        Some(entity)
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            for entity in cameras {
                main.entity_mut(entity)
                    .remove::<bevy::camera::RenderTarget>();
            }
            let mut views = main.resource_mut::<ManualTextureViews>();
            for target in &targets {
                views.remove(target);
            }
        }
        targets
    };
    world.resource_mut::<PreviewGpuState>().targets = targets;
}

// Manual targets belong to the device that produced them. A hidden preview
// must not keep its old attachment while its owner waits to render again.
fn clear_preview_gpu_targets(world: &mut World) {
    use bevy::{
        camera::NormalizedRenderTarget,
        render::{camera::ExtractedCamera, texture::ManualTextureViews},
    };
    // Extraction can queue old-device cameras while rendering is paused.
    world.schedule_scope(bevy::render::ExtractSchedule, |world, schedule| {
        schedule.apply_deferred(world);
    });
    if !std::mem::replace(
        &mut world.resource_mut::<PreviewGpuState>().initialized,
        true,
    ) {
        return;
    }
    let targets = {
        let mut state = world.resource_mut::<PreviewGpuState>();
        state.invalidate_main = true;
        state.targets.clone()
    };
    for target in &targets {
        world.resource_mut::<ManualTextureViews>().remove(target);
    }
    let cameras = world
        .query::<(Entity, &ExtractedCamera)>()
        .iter(world)
        .filter_map(|(entity, camera)| match camera.target {
            Some(NormalizedRenderTarget::TextureView(target)) if targets.contains(&target) => {
                Some(entity)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    for entity in cameras {
        world.entity_mut(entity).remove::<ExtractedCamera>();
    }
}

/// The previous render frame's pipeline state, available during panel sync.
/// Panel code only borrows the main world; it never reaches into the render app.
#[derive(Resource, Clone, Debug)]
pub struct PreviewPipelineStatus {
    pub pending: bool,
    pub error: Option<String>,
}

impl Default for PreviewPipelineStatus {
    fn default() -> Self {
        Self {
            pending: true,
            error: None,
        }
    }
}

fn publish_pipeline_status(world: &mut World) {
    use bevy::render::render_resource::{CachedPipelineState, PipelineCache};
    use bevy::shader::ShaderCacheError;
    let pipelines = world.resource::<PipelineCache>();
    let status = PreviewPipelineStatus {
        pending: pipelines.waiting_pipelines().next().is_some(),
        error: pipelines
            .pipelines()
            .find_map(|pipeline| match &pipeline.state {
                CachedPipelineState::Err(
                    ShaderCacheError::ShaderNotLoaded(_)
                    | ShaderCacheError::ShaderImportNotYetAvailable,
                ) => None,
                CachedPipelineState::Err(error) => Some(error.to_string()),
                _ => None,
            }),
    };
    world
        .resource_mut::<bevy::render::MainWorld>()
        .insert_resource(status);
}

// Bevy 0.20 resets GPU resources on recovery, but retains these per-entity GPU
// components. Inactive cameras keep clustering buffers that its upload system
// still visits, and light-probe extraction skips until new GPU images arrive.
// Remove only the derived GPU state; normal preparation rebuilds it while the
// main-world cameras, model assets and gameplay entities keep their identity.
#[cfg(feature = "scene")]
fn clear_scene_gpu_state(world: &mut World) {
    use bevy::{
        anti_alias::{
            smaa::{SmaaBindGroups, SmaaTextures, ViewSmaaPipelines},
            taa::{TemporalAntiAliasHistoryTextures, TemporalAntiAliasPipelineId},
        },
        core_pipeline::{
            prepass::ViewPrepassTextures,
            skybox::{SkyboxBindGroup, SkyboxPipelineId},
        },
        ecs::world::EntityRef,
        pbr::{
            MeshViewBindGroup, ScreenSpaceAmbientOcclusionResources, ShadowView, SsaoBindGroups,
            SsaoPipelineId, ViewClusterBindings, ViewShadowBindings, ViewTransmissionTexture,
            generate::{GeneratorBindGroups, IntermediateTextures, RenderEnvironmentMap},
        },
        render::view::{ViewDepthStencilTexture, ViewTarget},
    };
    // Preparation skips inactive views, but SSAO, SMAA and bloom bind-group
    // preparation can still visit their retained GPU textures. Clear their
    // complete device-dependent state, including history and pipeline IDs.
    // Keep the component list authoritative for both selection and removal.
    let mut gpu_components: Vec<_> = [
        world.component_id::<RenderEnvironmentMap>(),
        world.component_id::<IntermediateTextures>(),
        world.component_id::<GeneratorBindGroups>(),
        world.component_id::<ViewClusterBindings>(),
        world.component_id::<ViewShadowBindings>(),
        world.component_id::<MeshViewBindGroup>(),
        world.component_id::<ShadowView>(),
        world.component_id::<ViewTarget>(),
        world.component_id::<ViewDepthStencilTexture>(),
        world.component_id::<ViewTransmissionTexture>(),
        world.component_id::<ScreenSpaceAmbientOcclusionResources>(),
        world.component_id::<SsaoBindGroups>(),
        world.component_id::<SsaoPipelineId>(),
        world.component_id::<ViewPrepassTextures>(),
        world.component_id::<SmaaTextures>(),
        world.component_id::<SmaaBindGroups>(),
        world.component_id::<ViewSmaaPipelines>(),
        world.component_id::<TemporalAntiAliasHistoryTextures>(),
        world.component_id::<TemporalAntiAliasPipelineId>(),
        world.component_id::<SkyboxBindGroup>(),
        world.component_id::<SkyboxPipelineId>(),
    ]
    .into_iter()
    .flatten()
    .collect();
    // Clustering and atmosphere components live behind Bevy 0.20's private
    // modules. Bloom is optional and may be installed by the embedding host.
    // Exact registered names remove their state safely without representation
    // casts or another dependency. The scene feature keeps names in release
    // through bevy/debug. Remove this compatibility code when upstream resets
    // per-entity GPU state alongside its GPU resources.
    gpu_components.extend(
        world
            .components()
            .iter_registered()
            .filter(|(_, info)| {
                matches!(
                    &*info.name(),
                    "bevy_pbr::cluster::gpu::ViewGpuClusteringBuffers"
                        | "bevy_pbr::cluster::gpu::ViewClusteringBindGroups"
                        | "bevy_pbr::cluster::gpu::ViewClusteringDummyTexture"
                        | "bevy_pbr::cluster::gpu::ViewGpuClusteringPipelineIds"
                        | "bevy_pbr::atmosphere::resources::AtmosphereBuffer"
                        | "bevy_pbr::atmosphere::resources::AtmosphereTextures"
                        | "bevy_pbr::atmosphere::resources::AtmosphereBindGroups"
                        | "bevy_pbr::atmosphere::environment::AtmosphereProbeTextures"
                        | "bevy_pbr::atmosphere::environment::AtmosphereProbeBindGroups"
                        | "bevy_post_process::bloom::BloomTexture"
                        | "bevy_post_process::bloom::BloomBindGroups"
                )
            })
            .map(|(id, _)| id),
    );
    let entities: Vec<_> = world
        .query::<EntityRef>()
        .iter(world)
        .filter(|entity| gpu_components.iter().any(|id| entity.contains_id(*id)))
        .map(|entity| entity.id())
        .collect();
    for entity in entities {
        world.entity_mut(entity).remove_by_ids(&gpu_components);
    }
}

// Point/spot shadow preparation only recreates an existing shadow-view entity
// when its source light changes. Missing attachments here mean RenderStartup
// evicted old-device state. Keep requesting the rebuild until it can run,
// including when the first ready frames contain no active 3D views. Preserve
// the light's view-entity list; only its derived GPU attachments are replaced.
#[cfg(feature = "scene")]
fn rebuild_recovered_point_light_shadows(
    mut lights: Query<(
        &mut bevy::pbr::ExtractedPointLight,
        &bevy::pbr::PointAndSpotLightViewEntities,
    )>,
    shadow_views: Query<(), With<bevy::pbr::ShadowView>>,
) {
    for (mut light, views) in &mut lights {
        if views.iter().any(|view| shadow_views.get(*view).is_err()) {
            light.set_changed();
        }
    }
}

/// Explicit identity for Bed's preview entities. Gameplay can use its own
/// identities and query filters in the same world.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PreviewView(pub u64);

// Imported descendants can appear during SpawnScene. Assign their owner's
// layer before visibility runs, including on the first frame they exist.
fn inherit_preview_layers(world: &mut World) {
    use bevy::camera::visibility::RenderLayers;
    let children: Vec<_> = world
        .query_filtered::<Entity, (With<ChildOf>, Without<PreviewView>)>()
        .iter(world)
        .collect();
    let mut inherited = Vec::new();
    for child in children {
        let mut parent = world.get::<ChildOf>(child).map(ChildOf::parent);
        while let Some(entity) = parent {
            if let Some(view) = world.get::<PreviewView>(entity) {
                let layers = world
                    .get::<RenderLayers>(entity)
                    .expect("preview owner has a render layer")
                    .clone();
                inherited.push((child, *view, layers));
                break;
            }
            parent = world.get::<ChildOf>(entity).map(ChildOf::parent);
        }
    }
    for (entity, view, layers) in inherited {
        world.entity_mut(entity).insert((view, layers));
    }
}

#[derive(Resource, Default)]
struct PreviewLayers {
    assigned: BTreeMap<u64, usize>,
    free: BTreeSet<usize>,
    next: usize,
}

/// Layer zero belongs to ordinary gameplay. Preview cameras, lights and meshes
/// use their own layer, allocated once for each stable output identity.
pub fn preview_layers(world: &mut World, view: u64) -> bevy::camera::visibility::RenderLayers {
    let mut layers = world.resource_mut::<PreviewLayers>();
    let layer = if let Some(layer) = layers.assigned.get(&view) {
        *layer
    } else {
        let layer = if let Some(layer) = layers.free.pop_first() {
            layer
        } else {
            layers.next = layers.next.checked_add(1).expect("preview layer overflow");
            layers.next
        };
        layers.assigned.insert(view, layer);
        layer
    };
    bevy::camera::visibility::RenderLayers::layer(layer)
}

pub fn preview_target(world: &mut World, view: u64) -> bevy::camera::ManualTextureViewHandle {
    preview_layers(world, view);
    let layer = world.resource::<PreviewLayers>().assigned[&view];
    bevy::camera::ManualTextureViewHandle(u32::try_from(layer).expect("preview target overflow"))
}

/// Static/hidden outputs retain their texture. Only views synchronized by the
/// frontend this frame turn their camera back on.
pub fn begin_preview_frame(world: &mut World) {
    let mut cameras = world.query_filtered::<&mut Camera, With<PreviewView>>();
    for mut camera in cameras.iter_mut(world) {
        camera.is_active = false;
    }
}

/// Closing a panel releases its owned entities and target registration without
/// affecting unrelated game or editor entities in this world.
pub fn retire_previews(world: &mut World, live: &BTreeSet<u64>) {
    let dead: Vec<_> = world
        .resource::<PreviewLayers>()
        .assigned
        .iter()
        .filter(|(id, _)| !live.contains(id))
        .map(|(id, layer)| (*id, *layer))
        .collect();
    let entities: Vec<_> = world
        .query::<(Entity, &PreviewView)>()
        .iter(world)
        .filter(|(_, view)| !live.contains(&view.0))
        .map(|(entity, _)| entity)
        .collect();
    for entity in entities {
        // A retired parent recursively removes its collected descendants.
        let _ = world.try_despawn(entity);
    }
    for (id, layer) in dead {
        world
            .resource_mut::<bevy::render::texture::ManualTextureViews>()
            .remove(&bevy::camera::ManualTextureViewHandle(
                u32::try_from(layer).expect("valid target"),
            ));
        let mut layers = world.resource_mut::<PreviewLayers>();
        layers.assigned.remove(&id);
        layers.free.insert(layer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::camera::visibility::RenderLayers;

    #[test]
    fn recovery_invalidates_previews_only_when_the_main_world_is_available() {
        use bevy::camera::RenderTarget;
        use bevy::ecs::system::RunSystemOnce;
        use bevy::render::{ExtractSchedule, MainWorld, texture::ManualTextureViews};
        let mut render = World::new();
        render.init_resource::<PreviewGpuState>();
        render.init_resource::<ManualTextureViews>();
        render.add_schedule(bevy::ecs::schedule::Schedule::new(ExtractSchedule));
        // RenderStartup has no MainWorld, including its first device startup.
        clear_preview_gpu_targets(&mut render);
        assert!(!render.resource::<PreviewGpuState>().invalidate_main);

        let mut main = MainWorld::default();
        main.init_resource::<PreviewLayers>();
        main.init_resource::<ManualTextureViews>();
        let target = preview_target(&mut main, 10);
        let camera = main
            .spawn((
                Camera::default(),
                Projection::default(),
                RenderTarget::TextureView(target),
                PreviewView(10),
            ))
            .id();
        let gameplay = main
            .spawn((
                Camera::default(),
                Projection::default(),
                RenderTarget::None { size: UVec2::ONE },
            ))
            .id();
        render.insert_resource(main);
        extract_preview_gpu_state(&mut render);
        assert!(
            render
                .resource::<PreviewGpuState>()
                .targets
                .contains(&target)
        );
        assert!(
            render
                .resource::<MainWorld>()
                .get::<Camera>(camera)
                .unwrap()
                .is_active
        );
        assert!(
            render
                .resource::<MainWorld>()
                .get::<RenderTarget>(camera)
                .is_some()
        );

        let main = render.remove_resource::<MainWorld>().unwrap();
        clear_preview_gpu_targets(&mut render);
        assert!(render.resource::<PreviewGpuState>().invalidate_main);
        render.insert_resource(main);
        extract_preview_gpu_state(&mut render);
        let mut main = render.resource_mut::<MainWorld>();
        assert!(!main.get::<Camera>(camera).unwrap().is_active);
        assert!(main.get::<RenderTarget>(camera).is_none());
        assert!(main.get::<RenderTarget>(gameplay).is_some());
        assert!(main.entities().contains(camera));
        assert!(main.entities().contains(gameplay));
        main.init_resource::<Assets<bevy::image::Image>>();
        main.init_resource::<Messages<bevy::window::WindowResized>>();
        main.init_resource::<Messages<bevy::window::WindowCreated>>();
        main.init_resource::<Messages<bevy::window::WindowScaleFactorChanged>>();
        main.init_resource::<Messages<bevy::asset::AssetEvent<bevy::image::Image>>>();
        main.run_system_once::<_, (), _>(bevy::render::camera::camera_system)
            .unwrap();
        assert!(!render.resource::<PreviewGpuState>().invalidate_main);
    }

    #[test]
    fn late_imported_descendants_stay_in_their_preview() {
        let mut world = World::new();
        world.init_resource::<PreviewLayers>();
        let first = preview_layers(&mut world, 10);
        let second = preview_layers(&mut world, 20);
        let root = world.spawn((PreviewView(10), first.clone())).id();
        let branch = world.spawn(ChildOf(root)).id();
        let mesh = world.spawn(ChildOf(branch)).id();
        let game = world.spawn(RenderLayers::layer(0)).id();
        inherit_preview_layers(&mut world);
        assert_eq!(world.get::<PreviewView>(mesh), Some(&PreviewView(10)));
        assert_eq!(world.get::<RenderLayers>(mesh), Some(&first));
        assert!(!first.intersects(&second));
        assert!(!first.intersects(world.get::<RenderLayers>(game).unwrap()));

        let late = world.spawn(ChildOf(branch)).id();
        inherit_preview_layers(&mut world);
        assert_eq!(world.get::<RenderLayers>(late), Some(&first));
    }

    #[test]
    fn closing_one_preview_preserves_other_world_entities_and_reuses_its_layer() {
        let mut world = World::new();
        world.init_resource::<PreviewLayers>();
        world.init_resource::<bevy::render::texture::ManualTextureViews>();
        let first = preview_layers(&mut world, 10);
        let second = preview_layers(&mut world, 20);
        let closed = world.spawn((PreviewView(10), first.clone())).id();
        let child = world.spawn((PreviewView(10), ChildOf(closed))).id();
        let grandchild = world.spawn((PreviewView(10), ChildOf(child))).id();
        let open = world.spawn((PreviewView(20), second.clone())).id();
        let game = world.spawn_empty().id();
        retire_previews(&mut world, &BTreeSet::from([20]));
        assert!(world.get_entity(closed).is_err());
        assert!(world.get_entity(child).is_err());
        assert!(world.get_entity(grandchild).is_err());
        assert!(world.get_entity(open).is_ok());
        assert!(world.get_entity(game).is_ok());
        assert_eq!(preview_layers(&mut world, 30), first);
        assert_eq!(preview_layers(&mut world, 20), second);
    }
}
