//! Main-thread ImGui frontend. All domain resources stay installed while drawing.
use crate::editor_runtime::{
    EditorWorkspace, WorkbenchHostMode, process_editor_requests, publish_editor_frame,
};
use bed_config::Settings;
use bevy::prelude::*;
use dear_imgui_rs::Ui;
use std::io;

pub struct EditorUi {
    pub host_mode: WorkbenchHostMode,
}
impl EditorUi {
    pub fn new(host_mode: WorkbenchHostMode) -> Self {
        Self { host_mode }
    }
}

pub fn set_root_top_inset(world: &mut World, inset: f32) {
    world
        .resource_mut::<bed_workbench::WorkbenchViewport>()
        .top_inset = if inset.is_finite() {
        inset.max(0.0)
    } else {
        0.0
    };
}

/// A panel borrows only the resources it actually draws. Consequential
/// intentions return to the ordered reducer once every UI scope has ended.
pub fn draw_ui(world: &mut World, ui: &Ui) -> io::Result<()> {
    process_editor_requests(world)?;
    if let Some(textures) = world.get_resource::<bed_ui::gpu::CanvasTextures>() {
        let textures = textures.0.clone();
        world.resource_mut::<bed_editor_api::EditorFrame>().textures = textures;
    }
    publish_editor_frame(world);
    for mut visible in world
        .query::<&mut bed_ui::gpu::CanvasVisible>()
        .iter_mut(world)
    {
        visible.0 = false;
    }
    let font = world
        .get_non_send::<bed_ui::font::UiFonts>()
        .and_then(|fonts| fonts.main);
    let _font = font.map(|font| ui.push_font(font));
    let mode = world.non_send::<EditorUi>().host_mode;
    world
        .resource_mut::<bed_workbench::WorkbenchViewport>()
        .bounds = if mode == WorkbenchHostMode::Floating {
        let position = ui.cursor_screen_pos();
        let size = ui.content_region_avail();
        Some((position, size))
    } else {
        None
    };
    let status_height = crate::status_bar::height(ui);
    world
        .resource_mut::<bed_workbench::WorkbenchViewport>()
        .bottom_inset = status_height;
    let dirty = world
        .query::<(Entity, &bed_editor_api::EditorPanelBinding)>()
        .iter(world)
        .filter_map(|(entity, binding)| {
            world
                .get::<bed_workbench::PanelPresentation>(entity)
                .map(|_| {
                    (
                        entity,
                        binding
                            .document
                            .and_then(|document| {
                                world
                                    .non_send::<bed_document_session::EditorSession>()
                                    .snapshot(document)
                                    .ok()
                            })
                            .is_some_and(|snapshot| snapshot.dirty),
                    )
                })
        })
        .collect::<Vec<_>>();
    for (entity, dirty) in dirty {
        world
            .entity_mut(entity)
            .insert(bed_workbench::PanelDirty(dirty));
    }
    world.init_non_send::<bed_workbench::WorkbenchShortcuts>();
    let overrides = world.non_send::<Settings>().keybinds.overrides.clone();
    world
        .non_send_mut::<bed_workbench::WorkbenchShortcuts>()
        .overrides = overrides;
    bed_workbench::render_ui(world, ui);
    crate::status_bar::draw(world, ui);

    // A quarantined frontend cannot keep consequential intentions emitted
    // before its panic. Healthy surfaces retain their ordered intentions.
    let intents = world.resource_mut::<bed_editor_api::EditorIntents>().take();
    for intent in intents {
        if intent.source.is_some_and(|source| {
            world
                .get::<bed_workbench::PanelRecord>(source)
                .is_some_and(|record| record.unavailable.is_some())
        }) {
            continue;
        }
        if let Some(area) = intent.area {
            world
                .resource_mut::<bed_editor_api::EditorIntents>()
                .push_at(intent.source, area, intent.request);
        } else {
            world
                .resource_mut::<bed_editor_api::EditorIntents>()
                .push(intent.source, intent.request);
        }
    }
    let surfaces = world
        .query::<(
            &bed_workbench_api::PanelGeometry,
            &bed_workbench_api::PanelFrontend,
        )>()
        .iter(world)
        .filter_map(|(geometry, frontend)| geometry.visible.then_some(frontend.0))
        .collect::<Vec<_>>();
    for surface in surfaces {
        world
            .entity_mut(surface)
            .insert(bed_ui::gpu::CanvasVisible(true));
    }
    if let Some(error) = world.resource::<EditorWorkspace>().error.clone() {
        ui.window("Editor error")
            .flags(dear_imgui_rs::WindowFlags::ALWAYS_AUTO_RESIZE)
            .build(|| {
                ui.text_wrapped(&error);
                if ui.button("Dismiss") {
                    world.resource_mut::<EditorWorkspace>().error = None;
                }
            });
    }
    Ok(())
}
