//! Image presentation and managed preparation. File identity/lifecycle stays in Files.
use bedtime_editor2::TextBuffer;
use bedtime_image2::{DecodedImage, ImageContent, ImageView};
use bedtime_ui2::{UiContext, egui};
use bedtime_workbench2::*;
use bevy::prelude::*;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

type RenderedImage = Result<(Arc<ImageContent>, DecodedImage), String>;
struct RenderJob {
    stamp: Option<(DocumentId, u64)>,
    size: [u32; 2],
    task: ManagedTask<RenderedImage>,
}
#[derive(Component, Default)]
pub(crate) struct ImagePreview {
    pub view: ImageView,
    content: Option<Arc<ImageContent>>,
    observed: Option<(DocumentId, u64)>,
    rendered: Option<(DocumentId, u64)>,
    changed: Option<Instant>,
    job: Option<RenderJob>,
    rendered_size: [u32; 2],
    requested_size: [u32; 2],
    pub error: Option<String>,
}
impl Drop for ImagePreview {
    fn drop(&mut self) {
        if let Some(job) = &self.job {
            job.task.cancel();
        }
    }
}

pub(crate) fn accept_loaded(
    world: &mut World,
    panel: Entity,
    content: Arc<ImageContent>,
    pixels: DecodedImage,
) {
    if let Some(mut state) = world.get_mut::<ImagePreview>(panel) {
        state.view.set_image(pixels, content.dimensions());
        state.content = Some(content);
        state.error = None;
    }
}

pub(crate) fn open_result(world: &World, panel: Entity) -> Option<Result<(), String>> {
    let state = world.get::<ImagePreview>(panel)?;
    if let Some(error) = &state.error {
        return Some(Err(error.clone()));
    }
    state.view.has_image().then_some(Ok(()))
}

pub(crate) fn draw(ui: &mut egui::Ui, world: &mut World, _: &UiContext<'_>, panel: Entity) {
    let Some(mut state) = world.entity_mut(panel).take::<ImagePreview>() else {
        return;
    };
    if let Some(error) = &state.error {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
    let response = state.view.draw(ui, panel);
    world.entity_mut(panel).insert(state);
    response.context_menu(|ui| crate::opening::viewer_menu(ui, world, panel));
}

pub(crate) fn update(world: &mut World) {
    let panels: Vec<_> = world
        .query_filtered::<Entity, With<ImagePreview>>()
        .iter(world)
        .collect();
    for panel in panels {
        if world
            .get_resource::<ManagedWork>()
            .is_none_or(ManagedWork::is_closing)
        {
            continue;
        }
        let stamp = world.get::<DocumentView>(panel).and_then(|view| {
            inspect_document(world, view.document())
                .ok()
                .map(|info| (view.document(), info.revision))
        });
        let visible = world
            .get::<PanelPresentation>(panel)
            .is_some_and(|p| p.visible);
        let mut state = world.entity_mut(panel).take::<ImagePreview>().unwrap();
        let now = Instant::now();
        if stamp != state.observed {
            state.observed = stamp;
            state.changed = Some(now);
            state.error = None;
            if let Some(job) = state.job.take() {
                job.task.cancel();
            }
        }
        let desired = state.view.requested_size();
        if desired != state.requested_size {
            state.requested_size = desired;
            state.changed = Some(now);
        }
        if let Some(job) = &state.job {
            let result = match job.task.try_recv() {
                Ok(value) => Some(value),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    Some(Err("Image worker stopped".into()))
                }
            };
            if let Some(result) = result {
                let job = state.job.take().unwrap();
                if job.stamp == stamp {
                    state.rendered = stamp;
                    state.rendered_size = job.size;
                    match result {
                        Ok((content, pixels)) => {
                            state.view.set_image(pixels, content.dimensions());
                            state.content = Some(content);
                            state.error = None;
                        }
                        Err(error) => {
                            state.error = Some(error);
                        }
                    }
                }
            }
        }
        let source_changed = stamp.is_some() && stamp != state.rendered;
        let resolution_changed = state.content.as_ref().is_some_and(|c| c.is_svg())
            && desired != [0, 0]
            && desired != state.rendered_size;
        // Initial content must load even before layout presentation is published.
        let initial = !state.view.has_image();
        if (visible || initial)
            && state.job.is_none()
            && state.error.is_none()
            && (source_changed || resolution_changed)
            && (initial
                || state
                    .changed
                    .is_none_or(|t| now.duration_since(t) >= Duration::from_millis(150)))
        {
            let source = if source_changed {
                stamp
                    .and_then(|(id, _)| cast_document::<TextBuffer>(world, id).ok())
                    .and_then(|handle| read_document(world, handle).ok())
                    .cloned()
            } else {
                None
            };
            let cached = state.content.clone();
            let size = if desired == [0, 0] {
                [640, 480]
            } else {
                desired
            };
            let result = world.get_resource_mut::<ManagedWork>().unwrap().spawn_task(
                Some(panel),
                "Render image",
                move |cancel| {
                    if cancel.is_cancelled() {
                        return Err("Image rendering cancelled".into());
                    }
                    let content = match source {
                        Some(text) => Arc::new(ImageContent::prepare(
                            text.to_string().as_bytes(),
                            "preview.svg",
                        )?),
                        None => cached.ok_or("Image content is unavailable")?,
                    };
                    let dimensions = content.dimensions();
                    let width = size[0].max(1);
                    let height =
                        (width as f32 * dimensions.y / dimensions.x).ceil().max(1.0) as u32;
                    let pixels = content.render([width, height])?;
                    Ok((content, pixels))
                },
            );
            match result {
                Ok(task) => {
                    state.job = Some(RenderJob {
                        task,
                        stamp,
                        size: desired,
                    })
                }
                Err(error) => state.error = Some(error),
            }
        }
        world.entity_mut(panel).insert(state);
    }
}
