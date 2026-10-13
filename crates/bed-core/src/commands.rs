//! Ordinary document commands; their availability follows the installed models/views.
use crate::*;
use api::workbench::{CommandInvocation, CommandResult, WorkbenchAction};
use bed_documents::{DocumentId, DocumentKind, DocumentSession};
use bevy::prelude::*;

pub(crate) struct DocumentCommandsPlugin;
impl Plugin for DocumentCommandsPlugin {
    fn build(&self, app: &mut App) {
        let standard =
            *app.world().resource::<CorePresentationMode>() == CorePresentationMode::Standard;
        api::workbench::register(app.world_mut(), "bed.documents", |registrar| {
            registrar.command(
                "bed.documents.shell.open",
                "Open shell files",
                open_shell_files,
            );
            registrar.shell_alias("+open", "bed.documents.shell.open");
            registrar.shell_alias("+o", "bed.documents.shell.open");
            registrar.command("bed.documents.open_file", "Open File", open);
            registrar.command("bed.documents.save", "Save", save);
            registrar.command("bed.documents.save_as", "Save As", save_as);
            registrar.command("bed.documents.undo", "Undo document change", undo);
            registrar.command("bed.documents.redo", "Redo document change", redo);
            registrar.global_shortcut("bed.documents.open_file", &["Primary+O"]);
            registrar.global_shortcut("bed.documents.save", &["Primary+S"]);
            registrar.global_shortcut("bed.documents.save_as", &["Primary+Shift+S"]);
            registrar.command_icon("bed.documents.save", "bed.core.text");
            if standard {
                registrar.command("bed.documents.new", "New document", new_document);
                registrar.global_shortcut("bed.documents.new", &["Primary+N"]);
                registrar.command_icon("bed.documents.new", "bed.core.text");
                for id in [
                    "bed.documents.new",
                    "bed.documents.open_file",
                    "bed.documents.save",
                    "bed.documents.save_as",
                    "bed.documents.undo",
                    "bed.documents.redo",
                ] {
                    registrar.menu(id);
                }
            }
        })
        .expect("shared document commands are registered once");
        api::workbench::register(app.world_mut(), "bed.core", |registrar| {
            registrar.command("bed.core.close", "Close panel", close);
            registrar.command("bed.core.quit", "Quit application", quit_application);
            if standard {
                registrar.global_shortcut("bed.core.close", &["Primary+W"]);
                registrar.global_shortcut("bed.core.quit", &["Primary+Q"]);
                registrar.menu("bed.core.close");
            }
        })
        .expect("foundation lifecycle commands are registered once");
    }
}

fn open_shell_files(In(input): In<CommandInvocation>, world: &mut World) -> CommandResult {
    let shell = api::workbench::ShellInvocation::from_arguments(&input.arguments)?;
    if shell.arguments.is_empty() || shell.arguments.len() > 64 {
        return Err("usage: +open FILE ...".into());
    }
    if world.resource::<bed_files::FileServices>().is_remote() {
        return Err("Local shell files cannot be opened in an SSH context".into());
    }
    fn path(bytes: Vec<u8>) -> Result<std::path::PathBuf, String> {
        if bytes.is_empty() || bytes.contains(&0) {
            return Err("Shell paths must be nonempty and contain no NUL bytes".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            Ok(std::ffi::OsString::from_vec(bytes).into())
        }
        #[cfg(not(unix))]
        {
            String::from_utf8(bytes)
                .map(std::path::PathBuf::from)
                .map_err(|error| error.to_string())
        }
    }
    let directory = path(shell.directory)?;
    let paths = shell
        .arguments
        .into_iter()
        .map(|argument| path(argument).map(|argument| directory.join(argument)))
        .collect::<Result<Vec<_>, _>>()?;
    let area = world
        .resource::<bed_workbench::WorkbenchLayout>()
        .tiling
        .layout
        .largest_area();
    world.resource_mut::<api::features::HostEffects>().push_at(
        input.source,
        area,
        api::features::HostAction::OpenTerminalFiles { paths },
    );
    Ok(Vec::new())
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
        .resource::<bed_api::documents::DocumentFrame>()
        .active_document
}
fn save_target(world: &World, input: &CommandInvocation) -> Result<Option<DocumentId>, String> {
    // bEd and other command producers can snapshot an explicitly selected
    // document. An explicit empty target must not inherit a later selection.
    if let Some(document) = input.arguments.get("document") {
        if document.is_null() {
            return Ok(None);
        }
        let document = document
            .as_u64()
            .filter(|id| *id > 0)
            .map(DocumentId)
            .ok_or_else(|| "A captured document ID must be a positive integer".to_owned())?;
        crate::document_revision(world, document)?;
        return Ok(Some(document));
    }
    Ok(document(world, input).or_else(|| {
        world
            .get_resource::<DocumentFrameSelection>()
            .and_then(|selection| selection.document)
    }))
}

pub(crate) fn publish_document_commands(world: &mut World) {
    let features = world.resource::<bed_api::features::FeatureRegistry>();
    let has_editor = features.default_viewer(DocumentKind::Text).is_some();
    let has_viewer = !features.viewers().is_empty();
    let focused = world.resource::<bed_workbench::WorkbenchLayout>().focused;
    let input = CommandInvocation {
        command: String::new(),
        source: focused,
        area: 0,
        arguments: serde_json::Value::Null,
    };
    let has_document = document(world, &input).is_some();
    let has_save_target = save_target(world, &input).is_ok_and(|document| document.is_some());
    let local = !world.resource::<bed_files::FileServices>().is_remote();
    let mut facts = world.resource_mut::<api::workbench::CommandPresentations>();
    for id in ["bed.documents.new", "bed.documents.open_file"] {
        let available = if id == "bed.documents.open_file" {
            has_viewer && local
        } else {
            has_editor
        };
        facts.0.insert(
            id.into(),
            api::workbench::CommandPresentation {
                visible: if id == "bed.documents.open_file" {
                    has_viewer
                } else {
                    has_editor
                },
                enabled: available,
                ..Default::default()
            },
        );
    }
    for id in [
        "bed.documents.save",
        "bed.documents.save_as",
        "bed.documents.undo",
        "bed.documents.redo",
    ] {
        facts.0.insert(
            id.into(),
            api::workbench::CommandPresentation {
                visible: has_editor || has_document || has_save_target,
                enabled: if id == "bed.documents.save" || id == "bed.documents.save_as" {
                    has_save_target && (id != "bed.documents.save_as" || local)
                } else {
                    has_document
                },
                scope: if id == "bed.documents.save_as" && !local {
                    api::workbench::CommandPresentationScope::Global
                } else {
                    api::workbench::CommandPresentationScope::Invocation {
                        source: focused,
                        arguments: serde_json::Value::Null,
                    }
                },
                ..Default::default()
            },
        );
    }
    facts.0.insert(
        "bed.core.close".into(),
        api::workbench::CommandPresentation {
            enabled: focused.is_some(),
            scope: api::workbench::CommandPresentationScope::Invocation {
                source: focused,
                arguments: serde_json::Value::Null,
            },
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
    world.resource_mut::<api::features::HostEffects>().push_at(
        input.source,
        input.area,
        api::features::HostAction::OpenFileDialog { viewer: None },
    );
    Ok(Vec::new())
}
fn save(In(input): In<CommandInvocation>, world: &mut World) -> CommandResult {
    let document = save_target(world, &input)?.ok_or("No document is selected")?;
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
        world
            .resource_mut::<api::features::HostEffects>()
            .push(input.source, api::features::HostAction::Save { document });
        Ok(Vec::new())
    }
}
fn save_as(In(input): In<CommandInvocation>, world: &mut World) -> CommandResult {
    let document = save_target(world, &input)?.ok_or("No document is selected")?;
    world.resource_mut::<api::features::HostEffects>().push(
        input.source,
        api::features::HostAction::SaveAsDialog { document },
    );
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
    match quit(world) {
        Ok(()) => {
            world.write_message(AppExit::Success);
        }
        Err(_) if crate::documents::quit_save_pending(world) => {}
        Err(error) => return Err(error),
    }
    Ok(Vec::new())
}
