//! Host identity is permanent; the workbench renders plugin-owned contributions.
use bedtime_plugin_workspaces::{WorkspaceAttachment, WorkspaceSpec};
use bevy::prelude::*;
use std::path::Path;

fn host_information(spec: Option<&WorkspaceSpec>, directory: &Path) -> (String, String) {
    if let Some(spec) = spec {
        let location = spec.location();
        (
            spec.name.clone(),
            format!("Workspace: {}\nLocation: {location}", spec.name),
        )
    } else {
        let cwd = directory.to_string_lossy();
        (
            bedtime_plugin_workspaces::default_name(&cwd),
            format!("Working directory: {cwd}"),
        )
    }
}

pub(crate) fn publish_identity(world: &mut World) {
    let (summary, details) = host_information(
        world.resource::<WorkspaceAttachment>().saved(),
        &world.resource::<bedtime_files::FileContext>().directory,
    );
    world.insert_resource(bedtime_api::status::StatusBarIdentity {
        icon: "bedtime.core.folder".into(),
        summary,
        details,
    });
}

#[cfg(test)]
mod tests;
