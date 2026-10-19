use crate::*;

#[cfg(feature = "ui")]
pub(crate) fn rect(rect: Rect) -> Value {
    json!({"min":rect.min.to_array(),"max":rect.max.to_array()})
}
pub(crate) fn snapshot(world: &mut World, state: &State, entities: bool) -> Value {
    let windows: Vec<_> = world.query::<(Entity,&Window)>().iter(world).map(|(e,w)| json!({
        "entity":entity_id(e),"title":w.title,"size":[w.width(),w.height()],"physical_size":[w.physical_width(),w.physical_height()],"scale_factor":w.scale_factor(),"focused":w.focused,
    })).collect();
    let mut result = json!({"windows":windows});
    if let Some(render) = world.get_resource::<RenderProgress>() {
        result["render"] = json!({"frames":render.frames.load(Ordering::Acquire),"pending_pipelines":render.pending.load(Ordering::Acquire)});
    }
    #[cfg(feature = "ui")]
    {
        result["ui"] = json!({"frame":state.ui.frame,"surfaces":state.ui.surfaces,"texts":state.ui.texts.iter().map(|t|json!({"window":entity_id(t.window),"text":t.text,"rect":rect(t.rect)})).collect::<Vec<_>>()});
        result["viewports"] = json!(world.query::<(Entity,&bedtime_ui2::UiViewport)>().iter(world).map(|(e,v)| json!({"entity":entity_id(e),"visible":v.visible,"hovered":v.hovered,"rect":rect(v.rect),"physical_size":v.physical_size.to_array()})).collect::<Vec<_>>());
    }
    #[cfg(feature = "workbench")]
    if world.contains_resource::<bedtime_workbench2::WorkspaceRequests>() {
        result["workbench"] = crate::workbench::inspect(world);
    }
    if entities {
        result["entities"] = json!(world.iter_entities().map(|e| json!({"entity":entity_id(e.id()),"name":e.get::<Name>().map(|n|n.as_str()),"components":e.archetype().components().iter().filter_map(|id|world.components().get_info(*id).map(|i|i.name().to_string())).collect::<Vec<_>>()})).collect::<Vec<_>>());
    }
    let _ = state;
    result
}

#[cfg(feature = "ui")]
#[derive(Default)]
pub(crate) struct UiObservation {
    pub frame: u64,
    pub surfaces: Vec<Value>,
    pub texts: Vec<TextTarget>,
}
#[cfg(feature = "ui")]
pub(crate) struct TextTarget {
    pub window: Entity,
    pub text: String,
    pub rect: Rect,
}
#[cfg(feature = "ui")]
pub(crate) fn observe_ui(world: &mut World) {
    use bedtime_ui2::{UiCamera, UiCapture, UiDiagnostics};
    use bevy_egui::{EguiContext, EguiFullOutput};
    let mut observation = UiObservation::default();
    observation.frame = world.resource::<State>().frame;
    let cameras: Vec<_> = world
        .query::<(Entity, &UiCamera)>()
        .iter(world)
        .map(|(e, c)| (e, c.0))
        .collect();
    for (camera, window) in cameras {
        let Some(w) = world.get::<Window>(window) else {
            continue;
        };
        let scale = w.scale_factor();
        let pixels = world
            .get_mut::<EguiContext>(camera)
            .map(|mut ctx| ctx.get_mut().pixels_per_point())
            .unwrap_or(scale);
        let zoom = pixels / scale;
        let diagnostics = world.get::<UiDiagnostics>(window);
        let capture = world.get::<UiCapture>(window);
        observation.surfaces.push(json!({"window":entity_id(window),"camera":entity_id(camera),"pixels_per_point":pixels,"zoom":zoom,
            "diagnostics":diagnostics.map(|d|json!({"frames":d.frames,"vertices":d.vertices,"indices":d.indices,"error":d.error,"active_animations":d.active_animations,"icon_textures":d.icon_textures,"icon_rasterizations":d.icon_rasterizations})),
            "capture":capture.map(|c|json!({"keyboard":c.keyboard,"pointer":c.pointer,"text":c.text,"modal":c.modal})),
        }));
        if let Some(output) = world.get::<EguiFullOutput>(camera).and_then(|o| o.as_ref()) {
            for clipped in &output.shapes {
                text_shapes(
                    &clipped.shape,
                    clipped.clip_rect,
                    window,
                    zoom,
                    &mut observation.texts,
                );
            }
        }
    }
    world.resource_mut::<State>().ui = observation;
}
#[cfg(feature = "ui")]
fn text_shapes(
    shape: &bedtime_ui2::egui::Shape,
    clip: bedtime_ui2::egui::Rect,
    window: Entity,
    zoom: f32,
    out: &mut Vec<TextTarget>,
) {
    use bedtime_ui2::egui::Shape;
    match shape {
        Shape::Vec(shapes) => {
            for s in shapes {
                text_shapes(s, clip, window, zoom, out);
            }
        }
        Shape::Text(text) => {
            let visible = text
                .galley
                .rect
                .translate(text.pos.to_vec2())
                .intersect(clip);
            if visible.is_positive() && !text.galley.job.text.is_empty() {
                out.push(TextTarget {
                    window,
                    text: text.galley.job.text.clone(),
                    rect: Rect::from_corners(
                        Vec2::new(visible.min.x, visible.min.y) * zoom,
                        Vec2::new(visible.max.x, visible.max.y) * zoom,
                    ),
                });
            }
        }
        _ => (),
    }
}
