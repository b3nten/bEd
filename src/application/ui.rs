//! Main-thread ImGui frontend. All domain resources stay installed while drawing.
use crate::application::runtime::{
    WorkbenchHostMode, WorkspaceState, process_app_requests, publish_app_state,
};
use bevy::prelude::*;
use dear_imgui_rs::Ui;
use std::io;

pub struct AppUi {
    pub host_mode: WorkbenchHostMode,
}

impl AppUi {
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
    process_app_requests(world)?;
    bed_core::prepare_workbench_ui(world);
    publish_app_state(world);
    let font = world
        .get_non_send::<bed_ui::font::UiFonts>()
        .and_then(|fonts| fonts.main);
    let _font = font.map(|font| ui.push_font(font));
    let mode = world.non_send::<AppUi>().host_mode;
    world
        .resource_mut::<bed_workbench::WorkbenchViewport>()
        .bounds = if mode == WorkbenchHostMode::Floating {
        let position = ui.cursor_screen_pos();
        let size = ui.content_region_avail();
        Some((position, size))
    } else {
        None
    };
    let status_height = crate::application::status_bar::height(ui);
    world
        .resource_mut::<bed_workbench::WorkbenchViewport>()
        .bottom_inset = status_height;
    bed_workbench::render_ui(world, ui);
    crate::application::status_bar::draw(world, ui);

    bed_core::finish_workbench_ui(world);
    if let Some(error) = world.resource::<WorkspaceState>().error.clone() {
        ui.window("Application error")
            .flags(dear_imgui_rs::WindowFlags::ALWAYS_AUTO_RESIZE)
            .build(|| {
                ui.text_wrapped(&error);
                if ui.button("Dismiss") {
                    world.resource_mut::<WorkspaceState>().error = None;
                }
            });
    }
    Ok(())
}
