//! Version-one JSON boundary. Handles are opaque strings; validate their scope at use.
use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use bedtime_desktop::{DesktopInjectedEvent as Event, DesktopModifiers};
use image::ImageEncoder;
use serde_json::json;
use winit::{
    event::MouseButton,
    keyboard::{Key, KeyCode, NamedKey},
};

pub const BUILTINS: &[&str] = &[
    "describe",
    "inspect",
    "open_panel",
    "focus_panel",
    "split_panel",
    "close_panel",
    "invoke",
    "input",
    "capture",
    #[cfg(feature = "documents")]
    "read_document",
];

pub fn invalid(message: impl Into<String>) -> AutomationError {
    AutomationError::new(ErrorKind::InvalidRequest, message)
}
pub fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str, AutomationError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid(format!("{key} must be a string")))
}
pub fn panel_value(handle: PanelHandle) -> Value {
    json!(format!(
        "p:{}:{}:{}",
        handle.app,
        handle.session_id,
        handle.entity.to_bits()
    ))
}
fn handle_parts(value: &Value, kind: &str) -> Result<[u64; 3], AutomationError> {
    let text = value
        .as_str()
        .ok_or_else(|| invalid("handle must be a string"))?;
    let parts = text.split(':').collect::<Vec<_>>();
    if parts.len() != 4 || parts[0] != kind {
        return Err(invalid("invalid handle"));
    }
    Ok([parts[1], parts[2], parts[3]]
        .map(|part| part.parse::<u64>())
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| invalid("invalid handle"))?
        .try_into()
        .expect("three parts"))
}
pub fn panel(value: &Value) -> Result<PanelHandle, AutomationError> {
    let [app, session_id, bits] = handle_parts(value, "p")?;
    let entity = Entity::try_from_bits(bits).ok_or_else(|| invalid("invalid entity identity"))?;
    Ok(PanelHandle {
        app,
        session_id,
        entity,
    })
}
#[cfg(feature = "documents")]
pub fn document_value(handle: DocumentHandle) -> Value {
    json!(format!(
        "d:{}:{}:{}",
        handle.app, handle.session_id, handle.document.0
    ))
}
#[cfg(feature = "documents")]
pub fn document(value: &Value) -> Result<DocumentHandle, AutomationError> {
    let [app, session_id, id] = handle_parts(value, "d")?;
    Ok(DocumentHandle {
        app,
        session_id,
        document: bedtime_documents::DocumentId(id),
    })
}

pub fn decode(name: &str, args: Value) -> Result<Operation, AutomationError> {
    if !args.is_object() {
        return Err(invalid("arguments must be an object"));
    }
    let source = args
        .get("source")
        .filter(|value| !value.is_null())
        .map(panel)
        .transpose()?;
    Ok(match name {
        "describe" => Operation::Describe,
        "inspect" => Operation::Inspect,
        "open_panel" => Operation::OpenPanel {
            panel_type: string(&args, "panel_type")?.into(),
            state: args.get("state").cloned().unwrap_or(Value::Null),
            area: args
                .get("area")
                .map(|v| {
                    v.as_u64()
                        .and_then(|n| u32::try_from(n).ok())
                        .ok_or_else(|| invalid("invalid area"))
                })
                .transpose()?,
        },
        "focus_panel" => Operation::FocusPanel(panel(&args["panel"])?),
        "close_panel" => Operation::ClosePanel(panel(&args["panel"])?),
        "split_panel" => Operation::SplitPanel {
            panel: panel(&args["panel"])?,
            axis: args["axis"]
                .as_u64()
                .filter(|v| *v < 2)
                .ok_or_else(|| invalid("axis must be 0 or 1"))? as usize,
            high: args
                .get("high")
                .map(|v| v.as_bool().ok_or_else(|| invalid("high must be boolean")))
                .transpose()?
                .unwrap_or(true),
        },
        "invoke" => Operation::InvokeCommand {
            command: string(&args, "command")?.into(),
            source,
            arguments: args.get("arguments").cloned().unwrap_or(Value::Null),
        },
        "input" => {
            let steps = args["steps"]
                .as_array()
                .filter(|v| !v.is_empty() && v.len() <= 1024)
                .ok_or_else(|| invalid("input requires 1..1024 steps"))?;
            let mut decoded = Vec::new();
            for step in steps {
                let events = step
                    .as_array()
                    .filter(|v| v.len() <= 32)
                    .ok_or_else(|| invalid("each step requires at most 32 events"))?;
                decoded.push(DesktopInputStep {
                    events: events.iter().map(event).collect::<Result<_, _>>()?,
                });
            }
            Operation::Input(decoded)
        }
        "capture" => {
            let targets = args["targets"]
                .as_array()
                .filter(|v| !v.is_empty() && v.len() <= 8)
                .ok_or_else(|| invalid("capture requires 1..8 targets"))?;
            let targets = targets
                .iter()
                .map(|v| {
                    if v == "window" {
                        Ok(CaptureTarget::Window)
                    } else {
                        panel(v).map(CaptureTarget::Panel)
                    }
                })
                .collect::<Result<_, _>>()?;
            let timeout = args
                .get("timeout_ms")
                .map(|v| {
                    v.as_u64()
                        .filter(|n| *n > 0 && *n <= 90000)
                        .ok_or_else(|| invalid("timeout_ms must be 1..90000"))
                })
                .transpose()?
                .unwrap_or(10000);
            Operation::Capture {
                targets,
                after: args
                    .get("after")
                    .filter(|v| !v.is_null())
                    .map(|v| {
                        v.as_u64()
                            .map(OperationId)
                            .ok_or_else(|| invalid("after must be an operation ID"))
                    })
                    .transpose()?,
                timeout: Duration::from_millis(timeout),
            }
        }
        #[cfg(feature = "documents")]
        "read_document" => Operation::ReadDocument(document(&args["document"])?),
        name if name.contains('.') => Operation::Extension {
            name: name.into(),
            arguments: args,
        },
        _ => return Err(invalid(format!("unknown operation: {name}"))),
    })
}

fn pair(value: &Value) -> Result<[f32; 2], AutomationError> {
    let v = value
        .as_array()
        .filter(|v| v.len() == 2)
        .ok_or_else(|| invalid("expected two coordinates"))?;
    let mut result = [0.; 2];
    for (i, n) in v.iter().enumerate() {
        result[i] = n
            .as_f64()
            .filter(|n| n.is_finite() && n.abs() <= 1e6)
            .ok_or_else(|| invalid("invalid coordinate"))? as f32;
    }
    Ok(result)
}
fn event(v: &Value) -> Result<Event, AutomationError> {
    let pressed = || {
        v["pressed"]
            .as_bool()
            .ok_or_else(|| invalid("pressed must be boolean"))
    };
    Ok(match string(v, "type")? {
        "text" => Event::Text(string(v, "text")?.into()),
        "pointer" => Event::PointerMoved(pair(&v["position"])?),
        "scroll" => Event::Scroll(pair(&v["lines"])?),
        "button" => Event::PointerButton {
            button: match string(v, "button")? {
                "left" => MouseButton::Left,
                "right" => MouseButton::Right,
                "middle" => MouseButton::Middle,
                _ => return Err(invalid("unknown button")),
            },
            pressed: pressed()?,
        },
        "modifiers" => {
            let flag = |key| {
                v.get(key)
                    .map(|v| {
                        v.as_bool()
                            .ok_or_else(|| invalid("modifier must be boolean"))
                    })
                    .transpose()
                    .map(|v| v.unwrap_or(false))
            };
            Event::Modifiers(DesktopModifiers {
                shift: flag("shift")?,
                ctrl: flag("ctrl")?,
                alt: flag("alt")?,
                super_key: flag("super")?,
            })
        }
        "key" => {
            let physical: KeyCode = serde_json::from_value(v["physical"].clone())
                .map_err(|_| invalid("unknown physical key"))?;
            let logical = if let Some(named) = v.get("named") {
                Key::Named(
                    serde_json::from_value::<NamedKey>(named.clone())
                        .map_err(|_| invalid("unknown named key"))?,
                )
            } else {
                Key::Character(string(v, "character")?.into())
            };
            Event::Key {
                physical,
                logical,
                pressed: pressed()?,
            }
        }
        _ => return Err(invalid("unknown input event")),
    })
}

pub fn error(error: &AutomationError) -> Value {
    json!({"kind": format!("{:?}",error.kind), "message":error.message})
}
fn frame_info(info: &DesktopFrameInfo) -> Value {
    json!({"frame":info.frame.0,"generation":info.generation,"physical_size":info.physical_size,"display_pos":info.display_pos,"display_size":info.display_size,"framebuffer_scale":info.framebuffer_scale})
}
#[cfg(feature = "documents")]
fn document_info(info: &DocumentInfo) -> Value {
    json!({"handle":document_value(info.handle),"path":info.path,"kind":format!("{:?}",info.kind),"revision":info.revision,"dirty":info.dirty,"disk_conflict":info.disk_conflict})
}
pub fn encode(value: OperationValue) -> Result<Value, AutomationError> {
    Ok(match value {
        OperationValue::Json(value) => value,
        OperationValue::Applied => json!({"outcome":"applied"}),
        OperationValue::Ignored => json!({"outcome":"ignored"}),
        OperationValue::Opened(panel) => json!({"outcome":"opened","panel":panel_value(panel)}),
        OperationValue::Closed(panels) => {
            json!({"outcome":"closed","panels":panels.into_iter().map(panel_value).collect::<Vec<_>>()})
        }
        OperationValue::Input(receipt) => {
            json!({"last_ui_frame":receipt.last_ui_frame.0,"steps":receipt.steps})
        }
        OperationValue::Snapshot(snapshot) => {
            #[allow(unused_mut)]
            let mut value = json!({"frame":snapshot.frame.as_ref().map(frame_info),"layout":snapshot.layout,
                "panels":snapshot.panels.iter().map(|p| {
                    #[allow(unused_mut)]
                    let mut value=json!({"handle":panel_value(p.handle),"persisted_id":p.persisted_id,"panel_type":p.panel_type,"title":p.title,"area":p.area,"focused":p.focused,"available":p.available,"readiness":match &p.readiness { Readiness::Ready => json!({"state":"ready"}),Readiness::Pending => json!({"state":"pending"}),Readiness::Failed(e) => json!({"state":"failed","error":e}) },"visible_content":p.visible_content.map(|r| json!({"min":r.min,"max":r.max}))});
                    #[cfg(feature = "documents")]
                    { value["document"]=p.document.map(document_value).unwrap_or(Value::Null); }
                    value
                }).collect::<Vec<_>>(),
                "commands":snapshot.commands.iter().map(|c| json!({"id":c.id,"label":c.label,"enabled":c.enabled,"visible":c.visible})).collect::<Vec<_>>()});
            #[cfg(feature = "documents")]
            {
                value["documents"] = json!(
                    snapshot
                        .documents
                        .iter()
                        .map(document_info)
                        .collect::<Vec<_>>()
                );
            }
            value
        }
        #[cfg(feature = "documents")]
        OperationValue::Document(document) => {
            json!({"info":document_info(&document.info),"bytes":STANDARD.encode(document.bytes)})
        }
        OperationValue::Captured(captured) => {
            let total: usize = captured
                .images
                .iter()
                .map(|image| image.rgba.pixels.len())
                .sum();
            if total > 64 * 1024 * 1024 {
                return Err(invalid("capture exceeds 64 MiB pixel limit"));
            }
            let mut images = Vec::new();
            for image in captured.images {
                let mut png = Vec::new();
                image::codecs::png::PngEncoder::new(&mut png)
                    .write_image(
                        &image.rgba.pixels,
                        image.rgba.size[0],
                        image.rgba.size[1],
                        image::ExtendedColorType::Rgba8,
                    )
                    .map_err(|e| AutomationError::new(ErrorKind::Failed, e.to_string()))?;
                images.push(json!({"physical_rect":image.physical_rect,"size":image.rgba.size,"png":STANDARD.encode(png)}));
            }
            json!({"frame":frame_info(&captured.info),"images":images})
        }
    })
}
