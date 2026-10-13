//! Host identity is permanent; the workbench renders plugin-owned contributions.
use crate::runtime::WorkspaceState;
use bed_workbench::{StatusBarHost, render_status_bar};
use bevy::prelude::*;
use dear_imgui_rs::Ui;

pub(crate) fn height(ui: &Ui) -> f32 {
    bed_workbench::status_bar_height(ui)
}

fn host_information(workspace: &WorkspaceState) -> (String, String) {
    if let Some(spec) = &workspace.spec {
        let location = spec.location();
        (
            spec.name.clone(),
            format!("Workspace: {}\nLocation: {location}", spec.name),
        )
    } else {
        let cwd = workspace.working_directory.to_string_lossy();
        (
            bed_api::application::default_name(&cwd),
            format!("Working directory: {cwd}"),
        )
    }
}

pub(crate) fn draw(world: &mut World, ui: &Ui) {
    let (summary, details) = host_information(world.resource::<WorkspaceState>());
    render_status_bar(
        world,
        ui,
        StatusBarHost {
            icon: "folder",
            summary: &summary,
            details: &details,
        },
        |world, key| {
            world
                .get_non_send::<bed_ui::icons::Icons>()
                .and_then(|icons| icons.get(key))
        },
    );
}

#[cfg(test)]
mod tests;
