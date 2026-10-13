//! Ordinary document commands; their availability follows the installed models/views.
use crate::*;
use api::workbench::{CommandInvocation, CommandResult, WorkbenchAction};
use bed_documents::{DocumentId, DocumentKind, DocumentSession};
use bevy::prelude::*;

pub(crate) struct DocumentCommandsPlugin;
impl Plugin for DocumentCommandsPlugin {
    fn build(&self, app: &mut App) {
        api::workbench::register(app.world_mut(), "bed.core", |registrar| {
            registrar.command("bed.core.new", "New document", new_document);
            registrar.command("bed.core.open", "Open file…", open);
            registrar.command("bed.core.save", "Save document", save);
            registrar.command("bed.core.save_as", "Save document as…", save_as);
            registrar.command("bed.core.undo", "Undo document change", undo);
            registrar.command("bed.core.redo", "Redo document change", redo);
            registrar.command("bed.core.close", "Close panel", close);
            registrar.command("bed.core.quit", "Quit application", quit_application);
            registrar.global_shortcut("bed.core.new", &["Primary+N"]);
            registrar.global_shortcut("bed.core.open", &["Primary+O"]);
            registrar.global_shortcut("bed.core.save", &["Primary+S"]);
            registrar.global_shortcut("bed.core.save_as", &["Primary+Shift+S"]);
            registrar.global_shortcut("bed.core.close", &["Primary+W"]);
            for id in [
                "bed.core.new",
                "bed.core.open",
                "bed.core.save",
                "bed.core.save_as",
                "bed.core.undo",
                "bed.core.redo",
                "bed.core.close",
            ] {
                registrar.menu(id);
            }
            registrar.command_icon("bed.core.new", "bed.core.text");
            registrar.command_icon("bed.core.save", "bed.core.text");
        })
        .expect("core document commands are registered once");
        app.add_systems(PreUpdate, availability.in_set(CoreSystems::Frame));
    }
}
fn document(world: &World, input: &CommandInvocation) -> Option<DocumentId> {
    if let Some(source) = input.source {
        let source = world
            .get::<api::workbench::PanelFrontend>(source)
            .map_or(source, |frontend| frontend.0);
        return {
            world
                .get::<DocumentBinding>(source)
                .map(|binding| binding.0)
                .or_else(|| {
                    world
                        .get::<bed_api::documents::PanelBinding>(source)
                        .and_then(|binding| binding.document)
                })
        };
    }
    world
        .resource::<bed_api::features::HostFrame>()
        .active_document
}
fn availability(world: &mut World) {
    if *world.resource::<HostRequestMode>() == HostRequestMode::Application {
        for id in [
            "bed.core.new",
            "bed.core.open",
            "bed.core.save",
            "bed.core.save_as",
            "bed.core.undo",
            "bed.core.redo",
            "bed.core.close",
        ] {
            world
                .resource_mut::<api::workbench::CommandPresentations>()
                .0
                .insert(
                    id.into(),
                    api::workbench::CommandPresentation {
                        visible: false,
                        enabled: false,
                        ..Default::default()
                    },
                );
        }
        return;
    }
    let has_editor = world
        .resource::<bed_api::features::FeatureRegistry>()
        .default_viewer(DocumentKind::Text)
        .is_some();
    let focused = world.resource::<bed_workbench::WorkbenchLayout>().focused;
    let input = CommandInvocation {
        command: String::new(),
        source: focused,
        area: 0,
        arguments: serde_json::Value::Null,
    };
    let has_document = document(world, &input).is_some();
    let mut facts = world.resource_mut::<api::workbench::CommandPresentations>();
    for id in ["bed.core.new", "bed.core.open"] {
        facts.0.insert(
            id.into(),
            api::workbench::CommandPresentation {
                visible: has_editor,
                enabled: has_editor,
                ..Default::default()
            },
        );
    }
    for id in [
        "bed.core.save",
        "bed.core.save_as",
        "bed.core.undo",
        "bed.core.redo",
    ] {
        facts.0.insert(
            id.into(),
            api::workbench::CommandPresentation {
                visible: has_editor || has_document,
                enabled: has_document,
                ..Default::default()
            },
        );
    }
    facts.0.insert(
        "bed.core.close".into(),
        api::workbench::CommandPresentation {
            enabled: focused.is_some(),
            ..Default::default()
        },
    );
}
fn new_document(In(input): In<CommandInvocation>, world: &mut World) -> CommandResult {
    let viewer = world
        .resource::<bed_api::features::FeatureRegistry>()
        .default_viewer(DocumentKind::Text)
        .ok_or("No text editor is installed")?
        .clone();
    let document = world
        .non_send_mut::<DocumentSession>()
        .create_document(&[])
        .map_err(|error| error.to_string())?;
    open_document_panel(
        world,
        viewer.panel_type,
        document,
        serde_json::Value::Null,
        Some(input.area),
    )?;
    Ok(Vec::new())
}
fn open(In(input): In<CommandInvocation>, world: &mut World) -> CommandResult {
    if let Some(path) = rfd::FileDialog::new().pick_file() {
        open_file(world, &path, None, Some(input.area))?;
    }
    Ok(Vec::new())
}
fn save(In(input): In<CommandInvocation>, world: &mut World) -> CommandResult {
    let document = document(world, &input).ok_or("No document is selected")?;
    let needs_path = model_document(world, document)
        .map(|document| document.path.is_none())
        .unwrap_or_else(|| {
            world
                .non_send::<DocumentSession>()
                .snapshot(document)
                .is_ok_and(|snapshot| snapshot.path.is_empty())
        });
    if needs_path {
        save_as(In(input), world)
    } else {
        save_document(world, document)?;
        Ok(Vec::new())
    }
}
fn save_as(In(input): In<CommandInvocation>, world: &mut World) -> CommandResult {
    let document = document(world, &input).ok_or("No document is selected")?;
    if let Some(path) = rfd::FileDialog::new().save_file() {
        save_document_as(world, document, &path)?;
    }
    Ok(Vec::new())
}
fn undo(In(input): In<CommandInvocation>, world: &mut World) -> CommandResult {
    undo_document(
        world,
        document(world, &input).ok_or("No document is selected")?,
    )?;
    Ok(Vec::new())
}
fn redo(In(input): In<CommandInvocation>, world: &mut World) -> CommandResult {
    redo_document(
        world,
        document(world, &input).ok_or("No document is selected")?,
    )?;
    Ok(Vec::new())
}
fn close(In(input): In<CommandInvocation>) -> CommandResult {
    Ok(input
        .source
        .map(WorkbenchAction::ClosePanel)
        .into_iter()
        .collect())
}
fn quit_application(_: In<CommandInvocation>, world: &mut World) -> CommandResult {
    quit(world)?;
    world.write_message(AppExit::Success);
    Ok(Vec::new())
}
