//! Shared host operations use the document/settings owners, including their receipts.
use crate::{automation::*, bevy::prelude::*, core};
use base64::{Engine, engine::general_purpose::STANDARD};
use bedtime_api::{
    documents::PanelBinding,
    features::HostAction,
    settings::{SettingsRequest, SettingsRequestId, SettingsResults, SettingsSnapshot},
};
use serde_json::{Value, json};

pub(crate) fn register(world: &mut World) {
    for (name, description, submit, arguments) in [
        (
            "bedtime.open_file",
            "Open a file through the application host",
            open_file as fn(&mut World, Value) -> Result<ExtensionStart, AutomationError>,
            json!({"type":"object","required":["path"],"properties":{"path":{"type":"string"},"viewer":{"type":["string","null"]},"source":{"type":["string","null"]}}}),
        ),
        (
            "bedtime.apply_edits",
            "Apply byte edits at an explicit document revision",
            apply_edits,
            json!({"type":"object","required":["document","revision","edits"],"properties":{"document":{"type":"string"},"revision":{"type":"array","minItems":2,"maxItems":2,"items":{"type":"integer","minimum":0}},"edits":{"type":"array","maxItems":1024,"items":{"type":"object","required":["start","end","bytes"],"properties":{"start":{"type":"integer","minimum":0},"end":{"type":"integer","minimum":0},"bytes":{"type":"string","contentEncoding":"base64"}}}}}}),
        ),
        (
            "bedtime.save_documents",
            "Save explicit documents and await writes",
            save,
            json!({"type":"object","required":["documents"],"properties":{"documents":{"type":"array","maxItems":1024,"items":{"type":"string"}}}}),
        ),
        (
            "bedtime.save_as",
            "Save a document to a specified path",
            save_as,
            json!({"type":"object","required":["document","path"],"properties":{"document":{"type":"string"},"path":{"type":"string"}}}),
        ),
        (
            "bedtime.undo",
            "Undo shared document history",
            undo,
            json!({"type":"object","required":["document"],"properties":{"document":{"type":"string"}}}),
        ),
        (
            "bedtime.redo",
            "Redo shared document history",
            redo,
            json!({"type":"object","required":["document"],"properties":{"document":{"type":"string"}}}),
        ),
        (
            "bedtime.settings.set",
            "Change a registered setting through its owner",
            set_setting,
            json!({"type":"object","required":["owner","key","value"],"properties":{"owner":{"type":"string"},"key":{"type":"string"},"value":{}}}),
        ),
        (
            "bedtime.settings.inspect",
            "Read published settings",
            settings,
            json!({"type":"object"}),
        ),
        (
            "bedtime.focus_window",
            "Focus and show the native window",
            focus_window,
            json!({"type":"object"}),
        ),
        (
            "bedtime.quit",
            "Request orderly quit through the document owner",
            quit,
            json!({"type":"object"}),
        ),
    ] {
        register_extension(
            world,
            ExtensionDefinition {
                name,
                description,
                arguments,
                result: json!({"type":"object"}),
                submit,
            },
        )
        .expect("shared host extensions register once");
    }
}
fn source(world: &World, args: &Value) -> Result<Option<Entity>, AutomationError> {
    args.get("source")
        .filter(|v| !v.is_null())
        .map(|v| wire::panel(v).and_then(|p| resolve_panel(world, p)))
        .transpose()
}
fn doc(
    world: &World,
    value: &Value,
) -> Result<core::bedtime_documents::DocumentId, AutomationError> {
    resolve_document(world, wire::document(value)?)
}
fn host(
    world: &mut World,
    args: &Value,
    action: HostAction,
) -> Result<ExtensionStart, AutomationError> {
    let id = core::submit_host_request(world, source(world, args)?, action);
    Ok(ExtensionStart::pending(move |world| poll_host(world, id)))
}
fn poll_host(world: &mut World, id: core::HostRequestId) -> Option<Result<Value, AutomationError>> {
    core::take_host_result(world, id).map(|result| {
        let outcome = result.map_err(|error| AutomationError::new(ErrorKind::Failed, error))?;
        Ok(match outcome {
            core::HostOutcome::Opened(panel) => {
                let handle = panel_handle(world, panel)?;
                let surface = core::panel_presentation(world, panel).unwrap_or(panel);
                let document = world.get::<PanelBinding>(surface).and_then(|binding| binding.document);
                json!({"outcome":"opened", "panel":wire::panel_value(handle), "document":document.map(|id| wire::document_value(document_handle(world,id)))})
            }
            core::HostOutcome::Saved { documents, revisions } => {
                let saved = documents.into_iter().zip(revisions).map(|(id, revision)| {
                    json!({"document":wire::document_value(document_handle(world,id)),"revision":revision})
                }).collect::<Vec<_>>();
                json!({"outcome":"saved","documents":saved})
            }
            core::HostOutcome::Ignored => json!({"outcome":"ignored"}),
            _ => json!({"outcome":"applied"}),
        })
    })
}

fn open_file(world: &mut World, args: Value) -> Result<ExtensionStart, AutomationError> {
    let path = wire::string(&args, "path")?.into();
    let viewer = args
        .get("viewer")
        .filter(|v| !v.is_null())
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .ok_or_else(|| wire::invalid("viewer must be a string"))
        })
        .transpose()?;
    host(world, &args, HostAction::OpenFile { path, viewer })
}
fn apply_edits(world: &mut World, args: Value) -> Result<ExtensionStart, AutomationError> {
    let document = doc(world, &args["document"])?;
    let revision = serde_json::from_value(args["revision"].clone())
        .map_err(|_| wire::invalid("revision requires two integers"))?;
    let edits = args["edits"]
        .as_array()
        .filter(|v| v.len() <= 1024)
        .ok_or_else(|| wire::invalid("edits must be an array with at most 1024 entries"))?
        .iter()
        .map(|edit| {
            let start = edit["start"]
                .as_u64()
                .and_then(|v| usize::try_from(v).ok())
                .ok_or_else(|| wire::invalid("invalid edit start"))?;
            let end = edit["end"]
                .as_u64()
                .and_then(|v| usize::try_from(v).ok())
                .filter(|end| *end >= start)
                .ok_or_else(|| wire::invalid("invalid edit end"))?;
            let bytes = STANDARD
                .decode(wire::string(edit, "bytes")?)
                .map_err(|_| wire::invalid("invalid edit base64"))?;
            Ok(core::bedtime_documents::ByteEdit {
                range: start..end,
                bytes,
            })
        })
        .collect::<Result<Vec<_>, AutomationError>>()?;
    host(
        world,
        &args,
        HostAction::ApplyEdits {
            document,
            revision,
            edits,
        },
    )
}
fn save(world: &mut World, args: Value) -> Result<ExtensionStart, AutomationError> {
    let documents = args["documents"]
        .as_array()
        .filter(|v| v.len() <= 1024)
        .ok_or_else(|| wire::invalid("documents must be an array"))?
        .iter()
        .map(|v| doc(world, v))
        .collect::<Result<_, _>>()?;
    host(world, &args, HostAction::SaveDocuments { documents })
}
fn save_as(world: &mut World, args: Value) -> Result<ExtensionStart, AutomationError> {
    let document = doc(world, &args["document"])?;
    let path = wire::string(&args, "path")?.into();
    host(world, &args, HostAction::SaveAs { document, path })
}
fn undo(world: &mut World, args: Value) -> Result<ExtensionStart, AutomationError> {
    let document = doc(world, &args["document"])?;
    host(world, &args, HostAction::Undo { document })
}
fn redo(world: &mut World, args: Value) -> Result<ExtensionStart, AutomationError> {
    let document = doc(world, &args["document"])?;
    host(world, &args, HostAction::Redo { document })
}
fn settings(world: &mut World, _: Value) -> Result<ExtensionStart, AutomationError> {
    let snapshot = world.resource::<SettingsSnapshot>();
    Ok(ExtensionStart::Complete(
        json!({"settings":snapshot.settings,"revision":snapshot.revision,"error":snapshot.error}),
    ))
}
fn set_setting(world: &mut World, args: Value) -> Result<ExtensionStart, AutomationError> {
    let value = args
        .get("value")
        .cloned()
        .ok_or_else(|| wire::invalid("missing setting value"))?;
    let request = SettingsRequest::new(
        wire::string(&args, "owner")?,
        wire::string(&args, "key")?,
        value,
    )
    .tracked();
    let token = request.receipt.expect("tracked settings request").0;
    world.write_message(request);
    Ok(ExtensionStart::pending(move |world| {
        poll_setting(world, token)
    }))
}
fn poll_setting(world: &mut World, id: u64) -> Option<Result<Value, AutomationError>> {
    world
        .resource_mut::<SettingsResults>()
        .take(SettingsRequestId(id))
        .map(|result| {
            result
                .map(|revision| json!({"revision":revision}))
                .map_err(|e| AutomationError::new(ErrorKind::Failed, e))
        })
}
fn focus_window(world: &mut World, _: Value) -> Result<ExtensionStart, AutomationError> {
    let entity = world
        .query_filtered::<Entity, With<crate::bevy::window::PrimaryWindow>>()
        .iter(world)
        .next()
        .ok_or_else(|| AutomationError::new(ErrorKind::Unavailable, "native window unavailable"))?;
    crate::bevy::winit::WINIT_WINDOWS.with_borrow(|windows| {
        let window = windows.get_window(entity).ok_or_else(|| {
            AutomationError::new(ErrorKind::Unavailable, "native window unavailable")
        })?;
        window.set_visible(true);
        window.focus_window();
        Ok(())
    })?;
    Ok(ExtensionStart::Complete(json!({"outcome":"applied"})))
}
fn quit(world: &mut World, _: Value) -> Result<ExtensionStart, AutomationError> {
    let id = core::submit_quit(world);
    Ok(ExtensionStart::pending(move |world| poll_host(world, id)))
}
