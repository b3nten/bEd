//! Owned, pointer-free rendering for a separate render world or thread.

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use dear_imgui_rs::{
    Context, ContextId,
    render::{DetachedRendererConsumer, FrameSnapshot, SnapshotEpoch, TextureFeedback},
};

use super::{FramebufferExtent, WgpuRenderer, core::RendererContextState};
use crate::{
    ExternalTextureId, GammaMode, RendererError, RendererResult, ShaderManager, Uniforms,
    WgpuGpuData, WgpuInitInfo, WgpuTextureManager, wgpu,
};

struct SnapshotRendererIdentity {
    context: ContextId,
    generation: u64,
    active: AtomicBool,
    completion_watermark: AtomicU64,
}

/// UI-thread owner of the Dear ImGui state used by [`WgpuSnapshotRenderer`].
///
/// Keep this value alongside the [`Context`], and put only the GPU renderer and owned
/// [`FrameSnapshot`] values in the render world. Use [`Self::render_snapshot`] after building
/// the UI; it validates the renderer publications and polls earlier texture completions.
/// Neither the renderer nor a snapshot borrows this value or the Context.
///
/// Before replacing the renderer/device, return the GPU renderer to the UI thread and call
/// [`WgpuSnapshotRenderer::shutdown`]. Outstanding snapshots must first be rendered, reconciled,
/// or dropped. Dropping this owner ends the consumer generation and clears its publications.
pub struct WgpuSnapshotContext {
    state: Option<RendererContextState>,
    consumer: Option<DetachedRendererConsumer>,
    identity: Arc<SnapshotRendererIdentity>,
}

impl WgpuSnapshotContext {
    /// Returns the capability accepted by Context or FrameToken snapshot capture.
    ///
    /// When capturing through this lower-level API, call [`Self::poll_completions`] on the UI
    /// thread regularly so the renderer can prune completed texture retirement records.
    pub fn renderer_consumer(&self) -> RendererResult<&DetachedRendererConsumer> {
        self.state
            .as_ref()
            .ok_or(RendererError::ContextNotBound)?
            .ensure_renderer_contract()?;
        self.consumer.as_ref().ok_or(RendererError::ContextNotBound)
    }

    /// Polls completed snapshots and publishes their contiguous watermark to the renderer.
    pub fn poll_completions(&mut self, context: &mut Context) -> RendererResult<()> {
        self.state
            .as_ref()
            .ok_or(RendererError::ContextNotBound)?
            .ensure_matches(context)?;
        let progress = context.poll_snapshot_completions()?;
        self.identity
            .completion_watermark
            .fetch_max(progress.watermark(), Ordering::Release);
        Ok(())
    }

    /// Finishes the current UI frame into an owned, thread-safe snapshot.
    pub fn render_snapshot(&mut self, context: &mut Context) -> RendererResult<FrameSnapshot> {
        self.poll_completions(context)?;
        Ok(context.render_snapshot(self.renderer_consumer()?)?)
    }
}

impl Drop for WgpuSnapshotContext {
    fn drop(&mut self) {
        self.identity.active.store(false, Ordering::Release);
        // Clear published pointers before the boxed backend token is freed. The consumer's
        // own Drop starts draining outstanding completion tickets in the Context.
        self.state.take();
        self.consumer.take();
    }
}

/// GPU-only Dear ImGui renderer for owned [`FrameSnapshot`] values.
///
/// On native targets this type is `Send + Sync` even when the synchronous backend's window
/// features are enabled. It retains GPU handles and pointer-free texture identities only;
/// the matching [`WgpuSnapshotContext`] retains all UI-thread capabilities.
///
/// The caller owns render-pass creation, command submission and presentation. Snapshots must
/// arrive in epoch order. Dropped frames are allowed; applying texture requests with
/// [`Self::reconcile_snapshot`] avoids reuploading their data when drawing is skipped.
pub struct WgpuSnapshotRenderer {
    backend_data: Option<WgpuGpuData>,
    shader_manager: ShaderManager,
    texture_manager: WgpuTextureManager,
    default_texture: Option<wgpu::TextureView>,
    gamma_mode: GammaMode,
    identity: Arc<SnapshotRendererIdentity>,
}

impl WgpuSnapshotRenderer {
    /// Creates GPU resources and the matching UI-thread owner in one transaction.
    ///
    /// Window-specific initialization policy is not retained; detached rendering only targets
    /// caller-owned texture views and does not install platform-window rendering callbacks.
    pub fn new(
        init_info: WgpuInitInfo,
        context: &mut Context,
    ) -> RendererResult<(Self, WgpuSnapshotContext)> {
        WgpuRenderer::ensure_context_available(context)?;
        // Consumer Drop sends a detach message rather than mutating the UI Context. Merge a
        // previous owner's completed detach before the non-mutating admission preflight.
        context.poll_snapshot_completions()?;
        context.preflight_renderer_consumer()?;
        if let Some(adapter) = init_info.adapter.as_ref() {
            let features = adapter.get_texture_format_features(init_info.render_target_format);
            if !features
                .allowed_usages
                .contains(wgpu::TextureUsages::RENDER_ATTACHMENT)
                || !features
                    .flags
                    .contains(wgpu::TextureFormatFeatureFlags::BLENDABLE)
            {
                return Err(RendererError::InvalidRenderState(format!(
                    "Render target format {:?} requires RENDER_ATTACHMENT + BLENDABLE",
                    init_info.render_target_format,
                )));
            }
        }
        let mut backend_data = WgpuGpuData::new(&init_info);
        let mut shader_manager = ShaderManager::new();
        let default_texture =
            WgpuRenderer::create_device_objects_static(&mut shader_manager, &mut backend_data)?;
        let (flags, name) = WgpuRenderer::configure_imgui_context(context)?;
        let state = match RendererContextState::publish(context, flags) {
            Ok(state) => state,
            Err(error) => {
                WgpuRenderer::clear_unbound_imgui_context(context, flags, name);
                return Err(error);
            }
        };
        let consumer = match context.create_detached_renderer_consumer() {
            Ok(consumer) => consumer,
            Err(error) => {
                state.clear_with_context(context);
                return Err(error.into());
            }
        };
        let reset = match context.prepare_renderer_texture_reset(&consumer) {
            Ok(reset) => reset,
            Err(error) => {
                state.clear_with_context(context);
                return Err(error.into());
            }
        };
        reset.commit();
        let identity = Arc::new(SnapshotRendererIdentity {
            context: consumer.context_id(),
            generation: consumer.generation(),
            active: AtomicBool::new(true),
            completion_watermark: AtomicU64::new(0),
        });
        Ok((
            Self {
                backend_data: Some(backend_data),
                shader_manager,
                texture_manager: WgpuTextureManager::new(),
                default_texture: Some(default_texture),
                gamma_mode: GammaMode::Auto,
                identity: Arc::clone(&identity),
            },
            WgpuSnapshotContext {
                state: Some(state),
                consumer: Some(consumer),
                identity,
            },
        ))
    }

    fn ensure_active(&self) -> RendererResult<()> {
        if self.backend_data.is_none() || !self.identity.active.load(Ordering::Acquire) {
            return Err(RendererError::ContextNotBound);
        }
        Ok(())
    }

    fn ensure_epoch_matches(&self, epoch: SnapshotEpoch) -> RendererResult<()> {
        self.ensure_active()?;
        if epoch.context_id() != self.identity.context {
            return Err(RendererError::ContextMismatch);
        }
        if epoch.consumer_generation() != self.identity.generation {
            return Err(RendererError::InvalidRenderState(format!(
                "snapshot uses consumer generation {}, WGPU owns generation {}",
                epoch.consumer_generation(),
                self.identity.generation,
            )));
        }
        Ok(())
    }

    fn reconcile(&mut self, snapshot: &FrameSnapshot) -> RendererResult<Vec<TextureFeedback>> {
        self.ensure_epoch_matches(snapshot.epoch())?;
        let backend = self
            .backend_data
            .as_mut()
            .expect("active renderer has GPU data");
        let frame_count = i32::try_from(snapshot.draw_data().frame_count).map_err(|_| {
            RendererError::InvalidRenderState(
                "snapshot frame count exceeds native frame range".to_owned(),
            )
        })?;
        backend.begin_frame(snapshot.epoch().sequence(), frame_count)?;
        self.texture_manager.prune_destroyed_managed_textures(
            self.identity.completion_watermark.load(Ordering::Acquire),
        );
        self.texture_manager.handle_texture_requests(
            snapshot.texture_requests(),
            snapshot.epoch().sequence(),
            &backend.device,
            &backend.queue,
            &mut backend.render_resources,
        )
    }

    /// Applies and acknowledges managed textures without recording geometry.
    ///
    /// Use this when a surface cannot be acquired, the app is minimized, or drawing is skipped.
    pub fn reconcile_snapshot(&mut self, snapshot: FrameSnapshot) -> RendererResult<()> {
        let feedback = self.reconcile(&snapshot)?;
        snapshot.commit(feedback)?;
        Ok(())
    }

    /// Records one owned snapshot and acknowledges every managed texture request.
    ///
    /// Geometry never publishes or borrows Dear ImGui's transient Renderer_RenderState pointer.
    /// Standard reset/sampler commands are supported; arbitrary raw callbacks are rejected
    /// when the UI captures a snapshot. Texture feedback is committed even when draw validation
    /// fails, since those independent texture operations have already completed.
    pub fn render_snapshot(
        &mut self,
        snapshot: FrameSnapshot,
        render_pass: &mut wgpu::RenderPass<'_>,
        extent: FramebufferExtent,
    ) -> RendererResult<()> {
        let feedback = self.reconcile(&snapshot)?;
        let result = self.render_draw_data(snapshot.draw_data(), render_pass, extent);
        snapshot.commit(feedback)?;
        result
    }

    fn render_draw_data(
        &mut self,
        draw: &dear_imgui_rs::render::DrawDataSnapshot,
        render_pass: &mut wgpu::RenderPass<'_>,
        extent: FramebufferExtent,
    ) -> RendererResult<()> {
        if extent.is_empty() || draw.display_size[0] <= 0.0 || draw.display_size[1] <= 0.0 {
            return Ok(());
        }
        let backend = self
            .backend_data
            .as_mut()
            .expect("active renderer has GPU data");
        let prepared = WgpuRenderer::prepare_snapshot_draw_data(
            &self.texture_manager,
            &self.default_texture,
            draw,
            extent,
            backend,
        )?;
        if prepared.is_empty() {
            return Ok(());
        }
        let gamma = match self.gamma_mode {
            GammaMode::Auto => Uniforms::gamma_for_format(backend.render_target_format),
            GammaMode::Linear => 1.0,
            GammaMode::Gamma22 => 2.2,
        };
        let state = WgpuRenderer::prepare_snapshot_render_state(
            draw,
            backend,
            gamma,
            prepared.has_elements(),
        )?;
        WgpuRenderer::execute_prepared_snapshot_draw_data(prepared, &state, extent, render_pass);
        Ok(())
    }

    /// Sets the same output gamma policy as the synchronous backend.
    pub fn set_gamma_mode(&mut self, mode: GammaMode) {
        self.gamma_mode = mode;
    }

    /// Registers a cloned application-owned texture view for snapshot draw commands.
    pub fn register_external_texture(
        &mut self,
        view: &wgpu::TextureView,
    ) -> RendererResult<ExternalTextureId> {
        self.ensure_active()?;
        self.texture_manager.register_external_view(view)
    }

    /// Replaces a registered view, invalidating its cached image bind group.
    pub fn update_external_texture(
        &mut self,
        texture: ExternalTextureId,
        view: &wgpu::TextureView,
    ) -> RendererResult<()> {
        self.ensure_active()?;
        self.texture_manager.update_external_view(texture, view)?;
        self.backend_data
            .as_mut()
            .expect("active renderer has GPU data")
            .render_resources
            .remove_image_bind_group(texture.texture_id());
        Ok(())
    }

    /// Removes a registered view without destroying the application-owned texture.
    pub fn unregister_external_texture(
        &mut self,
        texture: ExternalTextureId,
    ) -> RendererResult<()> {
        self.ensure_active()?;
        self.texture_manager.remove_external_view(texture)?;
        self.backend_data
            .as_mut()
            .expect("active renderer has GPU data")
            .render_resources
            .remove_image_bind_group(texture.texture_id());
        Ok(())
    }

    /// Releases this renderer and its matching UI-thread owner for replacement/device recovery.
    ///
    /// This transaction rejects foreign contexts, mismatched renderer owners and outstanding
    /// snapshots before releasing resources. A rejected preflight leaves both halves intact.
    pub fn shutdown(
        &mut self,
        snapshot_context: &mut WgpuSnapshotContext,
        context: &mut Context,
    ) -> RendererResult<()> {
        if !Arc::ptr_eq(&self.identity, &snapshot_context.identity) {
            return Err(RendererError::ContextMismatch);
        }
        let state = snapshot_context
            .state
            .as_ref()
            .ok_or(RendererError::ContextNotBound)?;
        state.ensure_matches(context)?;
        let consumer = snapshot_context
            .consumer
            .as_ref()
            .ok_or(RendererError::ContextNotBound)?;
        let reset = context.prepare_renderer_texture_reset(consumer)?;
        self.backend_data.take();
        self.shader_manager = ShaderManager::new();
        self.texture_manager = WgpuTextureManager::new();
        self.default_texture.take();
        reset.commit();
        state.clear_with_context(context);
        self.identity.active.store(false, Ordering::Release);
        snapshot_context.state.take();
        snapshot_context.consumer.take();
        // Make the Context immediately reusable. No epoch can remain outstanding after the
        // reset permit above, so this only completes the consumer's detach notification.
        context.poll_snapshot_completions()?;
        Ok(())
    }
}
