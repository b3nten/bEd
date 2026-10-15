//! Ordinary document commands; their availability follows the installed models/views.
use crate::*;
use api::workbench::{CommandInvocation, CommandResult, WorkbenchAction};
use bedtime_documents::{DocumentId, DocumentKind, DocumentSession};
use bevy::prelude::*;

pub(crate) struct DocumentCommandsPlugin;
impl Plugin for DocumentCommandsPlugin {
    fn build(&self, app: &mut App) {
        api::workbench::register(app.world_mut(), "bedtime.documents", |registrar| {
            registrar.command(
                "bedtime.documents.shell.open",
                "Open shell files",
                open_shell_files,
            );
            registrar.shell_alias("+open", "bedtime.documents.shell.open");
            registrar.shell_alias("+o", "bedtime.documents.shell.open");
            registrar.command("bedtime.documents.open_file", "Open File", open);
            registrar.command("bedtime.documents.open_folder", "Open Folder", open_folder);
            registrar.command("bedtime.documents.save", "Save", save);
            registrar.command("bedtime.documents.save_as", "Save As", save_as);
            registrar.command("bedtime.documents.undo", "Undo document change", undo);
            registrar.command("bedtime.documents.redo", "Redo document change", redo);
            registrar.global_shortcut("bedtime.documents.open_file", &["Primary+O"]);
            registrar.global_shortcut("bedtime.documents.save", &["Primary+S"]);
            registrar.global_shortcut("bedtime.documents.save_as", &["Primary+Shift+S"]);
            registrar.command_icon("bedtime.documents.save", "text");
            registrar.command("bedtime.documents.new", "New document", new_document);
            registrar.global_shortcut("bedtime.documents.new", &["Primary+N"]);
            registrar.command_icon("bedtime.documents.new", "text");
            for id in [
                "bedtime.documents.new",
                "bedtime.documents.open_file",
                "bedtime.documents.save",
                "bedtime.documents.save_as",
                "bedtime.documents.undo",
                "bedtime.documents.redo",
            ] {
                registrar.menu(id);
            }
        })
        .expect("shared document commands are registered once");
        api::workbench::register(app.world_mut(), "bedtime.core", |registrar| {
            registrar.command("bedtime.core.close", "Close panel", close);
            registrar.command("bedtime.core.quit", "Quit application", quit_application);
            registrar.global_shortcut("bedtime.core.close", &["Primary+W"]);
            registrar.global_shortcut("bedtime.core.quit", &["Primary+Q"]);
            registrar.menu("bedtime.core.close");
        })
        .expect("foundation lifecycle commands are registered once");
        api::workbench::register(app.world_mut(), "bedtime.workbench", |registrar| {
            registrar.command("bedtime.workbench.split_right", "Split Right", split);
            registrar.command("bedtime.workbench.split_down", "Split Down", split);
            registrar.command(
                "bedtime.workbench.duplicate_view",
                "Duplicate View",
                duplicate,
            );
            registrar.global_shortcut("bedtime.workbench.split_right", &["Primary+Backslash"]);
        })
        .expect("shared workbench commands are registered once");
        api::workbench::register(app.world_mut(), "bedtime.edit", |registrar| {
            for (id, label) in [
                ("bedtime.edit.undo", "Undo"),
                ("bedtime.edit.redo", "Redo"),
                ("bedtime.edit.cut", "Cut"),
                ("bedtime.edit.copy", "Copy"),
                ("bedtime.edit.paste", "Paste"),
                ("bedtime.edit.select_all", "Select All"),
                ("bedtime.edit.find", "Find"),
                ("bedtime.edit.go_to_line", "Go to Line"),
            ] {
                registrar.command(id, label, panel_edit);
            }
        })
        .expect("native editing commands are registered once");
    }
}

fn open_shell_files(In(input): In<CommandInvocation>, world: &mut World) -> CommandResult {
    let shell = api::workbench::ShellInvocation::from_arguments(&input.arguments)?;
    if shell.arguments.is_empty() || shell.arguments.len() > 64 {
        return Err("usage: +open FILE ...".into());
    }
    if world.resource::<bedtime_files::FileServices>().is_remote() {
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
        .resource::<bedtime_workbench::WorkbenchLayout>()
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
    if let Some(document) = input.arguments.get("document") {
        return document.as_u64().filter(|id| *id > 0).map(DocumentId);
    }
    if let Some(source) = input.source {
        if let Some((document, _)) = crate::documents::panel_document_selection(world, source) {
            return Some(document);
        }
        if world
            .get::<bedtime_api::documents::PanelBinding>(source)
            .is_some_and(|binding| binding.input.local_file().is_some())
            || bedtime_api::workbench::panel_owner(world, source)
                .and_then(|owner| crate::panel_presentation(world, owner))
                .is_some_and(|frontend| {
                    world
                        .get::<bedtime_api::documents::PanelBinding>(frontend)
                        .is_some_and(|binding| binding.input.local_file().is_some())
                })
        {
            return None;
        }
    }
    world
        .resource::<bedtime_api::documents::DocumentFrame>()
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
    let features = world.resource::<bedtime_api::features::FeatureRegistry>();
    let has_editor = features.default_viewer(DocumentKind::Text).is_some();
    let has_viewer = !features.viewers().is_empty();
    let focused = world
        .resource::<bedtime_workbench::WorkbenchLayout>()
        .focused;
    let input = CommandInvocation {
        command: bedtime_api::CommandId::new(""),
        source: focused,
        area: 0,
        arguments: serde_json::Value::Null,
    };
    let has_document = document(world, &input).is_some();
    let selected = save_target(world, &input).ok().flatten();
    let has_save_target = selected.is_some();
    let can_save_as = selected.is_none_or(|document| !model_document_has_writer(world, document));
    let local = !world.resource::<bedtime_files::FileServices>().is_remote();
    let mut facts = world.resource_mut::<api::workbench::CommandPresentations>();
    for id in ["bedtime.documents.new", "bedtime.documents.open_file"] {
        let available = if id == "bedtime.documents.open_file" {
            has_viewer && local
        } else {
            has_editor
        };
        facts.0.insert(
            id.into(),
            api::workbench::CommandPresentation {
                visible: if id == "bedtime.documents.open_file" {
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
        "bedtime.documents.save",
        "bedtime.documents.save_as",
        "bedtime.documents.undo",
        "bedtime.documents.redo",
    ] {
        facts.0.insert(
            id.into(),
            api::workbench::CommandPresentation {
                visible: has_editor || has_document || has_save_target,
                enabled: if id == "bedtime.documents.save" || id == "bedtime.documents.save_as" {
                    has_save_target && (id != "bedtime.documents.save_as" || (local && can_save_as))
                } else {
                    has_document
                },
                scope: if id == "bedtime.documents.save_as" && (!local || !can_save_as) {
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
        "bedtime.core.close".into(),
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
    world.resource_mut::<api::features::HostEffects>().push_at(
        input.source,
        input.area,
        api::features::HostAction::NewDocument,
    );
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
    let needs_path = !model_document_has_writer(world, document)
        && model_document(world, document)
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
    if model_document_has_writer(world, document) {
        return Err("This document saves through its application; Save As is unavailable".into());
    }
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
    submit_quit(world);
    Ok(Vec::new())
}

fn open_folder(In(input): In<CommandInvocation>, world: &mut World) -> CommandResult {
    world
        .resource_mut::<api::features::HostEffects>()
        .push(input.source, api::features::HostAction::ChooseDirectory);
    Ok(Vec::new())
}

fn split(In(input): In<CommandInvocation>) -> CommandResult {
    Ok(input
        .source
        .map(|panel| WorkbenchAction::SplitPanel {
            panel,
            axis: usize::from(input.command.ends_with("split_down")),
            high: true,
        })
        .into_iter()
        .collect())
}

fn duplicate(In(input): In<CommandInvocation>, world: &mut World) -> CommandResult {
    if let Some(panel) = input.source {
        world.resource_mut::<api::features::HostEffects>().push_at(
            input.source,
            input.area,
            api::features::HostAction::DuplicatePanel { panel },
        );
    }
    Ok(Vec::new())
}

fn panel_edit(In(input): In<CommandInvocation>, world: &mut World) -> CommandResult {
    let Some(panel) = input.source else {
        return Ok(Vec::new());
    };
    use api::workbench::{ClipboardAction, PanelAction};
    let action = match input.command.as_str() {
        "bedtime.edit.undo" => api::features::HostAction::Undo {
            document: document(world, &input).ok_or("No document is selected")?,
        },
        "bedtime.edit.redo" => api::features::HostAction::Redo {
            document: document(world, &input).ok_or("No document is selected")?,
        },
        "bedtime.edit.cut" => api::features::HostAction::Clipboard {
            panel,
            action: ClipboardAction::Cut,
        },
        "bedtime.edit.copy" => api::features::HostAction::Clipboard {
            panel,
            action: ClipboardAction::Copy,
        },
        "bedtime.edit.paste" => api::features::HostAction::Clipboard {
            panel,
            action: ClipboardAction::PasteFromClipboard(None),
        },
        "bedtime.edit.select_all" => api::features::HostAction::PanelAction {
            panel,
            action: PanelAction::SelectAll,
        },
        "bedtime.edit.find" => api::features::HostAction::PanelAction {
            panel,
            action: PanelAction::Find,
        },
        "bedtime.edit.go_to_line" => api::features::HostAction::PanelAction {
            panel,
            action: PanelAction::GoToLine,
        },
        _ => return Err("Unknown editing command".into()),
    };
    world
        .resource_mut::<api::features::HostEffects>()
        .push(input.source, action);
    Ok(Vec::new())
}
