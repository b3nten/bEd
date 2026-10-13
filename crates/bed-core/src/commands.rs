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
        api::workbench::register(app.world_mut(), "bed.core", |registrar| {
            registrar.command("bed.core.new", "New document", new_document);
            registrar.command("bed.core.open", "Open file…", open);
            registrar.command("bed.core.shell.open", "Open shell files", open_shell_files);
            registrar.shell_alias("+open", "bed.core.shell.open");
            registrar.shell_alias("+o", "bed.core.shell.open");
            registrar.command("bed.core.save", "Save document", save);
            registrar.command("bed.core.save_as", "Save document as…", save_as);
            registrar.command("bed.core.undo", "Undo document change", undo);
            registrar.command("bed.core.redo", "Redo document change", redo);
            registrar.command("bed.core.close", "Close panel", close);
            registrar.command("bed.core.quit", "Quit application", quit_application);
            if standard {
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
            }
            registrar.command_icon("bed.core.new", "bed.core.text");
            registrar.command_icon("bed.core.save", "bed.core.text");
        })
        .expect("core document commands are registered once");
        app.add_systems(PreUpdate, availability.in_set(CoreSystems::Frame));
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
    for path in paths {
        crate::open_file(world, &path, None, Some(area))?;
    }
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
fn availability(world: &mut World) {
    if *world.resource::<CorePresentationMode>() == CorePresentationMode::Application {
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
    let local = !world.resource::<bed_files::FileServices>().is_remote();
    let mut facts = world.resource_mut::<api::workbench::CommandPresentations>();
    for id in ["bed.core.new", "bed.core.open"] {
        facts.0.insert(
            id.into(),
            api::workbench::CommandPresentation {
                visible: has_editor,
                enabled: has_editor && (id != "bed.core.open" || local),
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
                enabled: has_document && (id != "bed.core.save_as" || local),
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
    world.resource_mut::<api::features::HostEffects>().push_at(
        input.source,
        input.area,
        api::features::HostAction::OpenFileDialog { viewer: None },
    );
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
        world
            .resource_mut::<api::features::HostEffects>()
            .push(input.source, api::features::HostAction::Save { document });
        Ok(Vec::new())
    }
}
fn save_as(In(input): In<CommandInvocation>, world: &mut World) -> CommandResult {
    let document = document(world, &input).ok_or("No document is selected")?;
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
