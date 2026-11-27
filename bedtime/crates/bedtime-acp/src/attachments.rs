//! File references and image encoding for ACP prompts.
use crate::worker::AgentCapabilitiesSummary;
use agent_client_protocol::v1::schema::{
    ContentBlock, EmbeddedResource, EmbeddedResourceResource, ImageContent, ResourceLink,
    TextContent, TextResourceContents,
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use bedtime_clipboard::ClipboardImage;
use bedtime_workbench::{BedtimeWorldExt as _, TextBuffer};
use bevy::prelude::World;
use image::{ColorType, ImageEncoder as _, codecs::png::PngEncoder};
use std::path::{Path, PathBuf};

/// Maximum UTF-8 file size to embed directly when the agent advertises
/// `promptCapabilities.embeddedContext`. Larger files are sent as `ResourceLink`.
const MAX_EMBEDDED_FILE_BYTES: u64 = 256 * 1024;

/// Maximum raw image size to read from disk.
const MAX_IMAGE_FILE_BYTES: u64 = 20 * 1024 * 1024;

/// The text of files that are open as documents, by path.
pub type OpenTexts = std::collections::HashMap<PathBuf, String>;

/// The open documents' text for the files among `attachments`.
pub fn open_texts(world: &World, attachments: &[PromptAttachment]) -> OpenTexts {
    let bedtime = world.bedtime();
    attachments
        .iter()
        .filter_map(|attachment| match attachment {
            PromptAttachment::File { path, .. } => Some(path),
            PromptAttachment::Image { .. } => None,
        })
        .filter_map(|path| {
            let document = bedtime.document_for_file::<TextBuffer>(path)?;
            let text = bedtime.read_document(document).ok()?;
            Some((path.clone(), text.to_string()))
        })
        .collect()
}

/// An attachment queued with a prompt (a workspace/disk file or an image).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromptAttachment {
    File {
        path: PathBuf,
        display_name: String,
    },
    Image {
        label: String,
        mime_type: String,
        base64_data: String,
        path: Option<PathBuf>,
    },
}

impl PromptAttachment {
    /// Human-readable label shown in attachment chips and transcript badges.
    pub fn label(&self) -> &str {
        match self {
            Self::File { display_name, .. } => display_name,
            Self::Image { label, .. } => label,
        }
    }

    pub fn is_image(&self) -> bool {
        matches!(self, Self::Image { .. })
    }

    /// Create a file or image attachment from a path on disk.
    pub fn from_path(path: PathBuf, cwd: &Path) -> Result<Self, String> {
        let display_name = relative_display_name(&path, cwd);
        if let Some(mime_type) = image_mime_for_path(&path) {
            let meta = std::fs::metadata(&path)
                .map_err(|e| format!("Couldn't read {}: {e}", path.display()))?;
            if meta.len() > MAX_IMAGE_FILE_BYTES {
                return Err(format!(
                    "Image {} is too large (max {} MB).",
                    display_name,
                    MAX_IMAGE_FILE_BYTES / (1024 * 1024)
                ));
            }
            let bytes = std::fs::read(&path)
                .map_err(|e| format!("Couldn't read {}: {e}", path.display()))?;
            let base64_data = BASE64.encode(bytes);
            return Ok(Self::Image {
                label: display_name,
                mime_type: mime_type.to_owned(),
                base64_data,
                path: Some(path),
            });
        }

        Ok(Self::File { path, display_name })
    }

    /// PNG-encode an RGBA clipboard image into an image attachment.
    pub fn from_clipboard_image(img: &ClipboardImage) -> Result<Self, String> {
        let width = u32::try_from(img.width).map_err(|_| "Clipboard image too wide".to_owned())?;
        let height =
            u32::try_from(img.height).map_err(|_| "Clipboard image too tall".to_owned())?;
        let expected_len = img.width.saturating_mul(img.height).saturating_mul(4);
        if img.rgba.len() != expected_len {
            return Err("Clipboard image pixel buffer size mismatch.".into());
        }
        let mut png_bytes = Vec::new();
        let encoder = PngEncoder::new(&mut png_bytes);
        encoder
            .write_image(&img.rgba, width, height, ColorType::Rgba8.into())
            .map_err(|e| format!("Couldn't encode clipboard image as PNG: {e}"))?;
        let base64_data = BASE64.encode(png_bytes);
        Ok(Self::Image {
            label: format!("Clipboard ({width}×{height})"),
            mime_type: "image/png".to_owned(),
            base64_data,
            path: None,
        })
    }
}

/// Convert prompt text + attachments into ACP v1 `ContentBlock`s respecting
/// the connected agent's advertised `PromptCapabilities`.
///
/// `open` has the text of the attached files that are open as documents: a
/// file is sent as its editor shows it, unsaved changes and all.
pub fn build_prompt_blocks(
    text: &str,
    attachments: &[PromptAttachment],
    caps: AgentCapabilitiesSummary,
    open: &OpenTexts,
) -> Result<Vec<ContentBlock>, String> {
    let mut blocks = Vec::with_capacity(attachments.len() + 1);

    for attachment in attachments {
        match attachment {
            PromptAttachment::File { path, display_name } => {
                let uri = path_to_file_uri(path);
                let mime = text_mime_for_path(path).map(str::to_owned);
                let meta = std::fs::metadata(path).ok();
                let size = meta.as_ref().and_then(|m| i64::try_from(m.len()).ok());

                let contents = match open.get(path) {
                    Some(text) => Some(text.clone())
                        .filter(|text| text.len() as u64 <= MAX_EMBEDDED_FILE_BYTES),
                    None => meta
                        .as_ref()
                        .filter(|m| m.len() <= MAX_EMBEDDED_FILE_BYTES)
                        .and_then(|_| std::fs::read_to_string(path).ok()),
                };
                if caps.supports_embedded_context
                    && let Some(contents) = contents
                {
                    let resource = TextResourceContents::new(contents, uri).mime_type(mime);
                    blocks.push(ContentBlock::Resource(EmbeddedResource::new(
                        EmbeddedResourceResource::TextResourceContents(resource),
                    )));
                } else {
                    let link = ResourceLink::new(display_name.clone(), uri)
                        .mime_type(mime)
                        .size(size);
                    blocks.push(ContentBlock::ResourceLink(link));
                }
            }
            PromptAttachment::Image {
                label,
                mime_type,
                base64_data,
                path,
            } => {
                if caps.supports_image {
                    let uri = path.as_deref().map(path_to_file_uri);
                    let img = ImageContent::new(base64_data.clone(), mime_type.clone()).uri(uri);
                    blocks.push(ContentBlock::Image(img));
                } else if let Some(path) = path {
                    // Agent doesn't advertise image blocks, fall back to a file resource link.
                    let uri = path_to_file_uri(path);
                    let link =
                        ResourceLink::new(label.clone(), uri).mime_type(Some(mime_type.clone()));
                    blocks.push(ContentBlock::ResourceLink(link));
                } else {
                    return Err(
                        "This agent does not advertise image support in ACP promptCapabilities."
                            .into(),
                    );
                }
            }
        }
    }

    if !text.trim().is_empty() {
        blocks.push(ContentBlock::Text(TextContent::new(text.to_owned())));
    }

    Ok(blocks)
}

fn relative_display_name(path: &Path, cwd: &Path) -> String {
    if let Ok(rel) = path.strip_prefix(cwd) {
        rel.display().to_string()
    } else {
        path.file_name()
            .and_then(|n| n.to_str())
            .map(str::to_owned)
            .unwrap_or_else(|| path.display().to_string())
    }
}

/// Convert a filesystem path to a `file://` URI with percent-encoding.
pub fn path_to_file_uri(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let mut encoded = String::with_capacity(raw.len() + 8);
    encoded.push_str("file://");
    if !raw.starts_with('/') {
        encoded.push('/');
    }
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                encoded.push(byte as char);
            }
            other => {
                encoded.push_str(&format!("%{other:02X}"));
            }
        }
    }
    encoded
}

fn image_mime_for_path(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        "gif" => Some("image/gif"),
        _ => None,
    }
}

fn text_mime_for_path(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "rs" => Some("text/x-rust"),
        "md" | "markdown" => Some("text/markdown"),
        "json" => Some("application/json"),
        "toml" => Some("application/toml"),
        "yaml" | "yml" => Some("application/yaml"),
        "js" | "mjs" | "cjs" => Some("text/javascript"),
        "ts" | "tsx" => Some("text/typescript"),
        "py" => Some("text/x-python"),
        "html" | "htm" => Some("text/html"),
        "css" => Some("text/css"),
        "txt" | "log" => Some("text/plain"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_uri_encodes_spaces() {
        let uri = path_to_file_uri(Path::new("/tmp/my folder/hello world.rs"));
        assert_eq!(uri, "file:///tmp/my%20folder/hello%20world.rs");
    }

    #[test]
    fn an_open_file_is_sent_as_its_document_has_it() {
        let path = PathBuf::from("/nowhere/open.rs");
        let attachment = PromptAttachment::File {
            path: path.clone(),
            display_name: "open.rs".into(),
        };
        let caps = AgentCapabilitiesSummary {
            supports_embedded_context: true,
            ..Default::default()
        };
        let open = OpenTexts::from([(path, "unsaved text".to_owned())]);
        let blocks = build_prompt_blocks("", &[attachment.clone()], caps, &open).unwrap();
        let [ContentBlock::Resource(resource)] = blocks.as_slice() else {
            panic!("not one embedded resource: {blocks:?}");
        };
        let EmbeddedResourceResource::TextResourceContents(contents) = &resource.resource else {
            panic!("not text");
        };
        assert_eq!(contents.text, "unsaved text");
        // An agent that doesn't take embedded files gets a link, as before.
        let caps = AgentCapabilitiesSummary::default();
        let blocks = build_prompt_blocks("", &[attachment], caps, &open).unwrap();
        assert!(matches!(blocks.as_slice(), [ContentBlock::ResourceLink(_)]));
    }

    #[test]
    fn clipboard_image_encodes_to_png() {
        let img = ClipboardImage {
            width: 2,
            height: 2,
            rgba: vec![255; 16],
        };
        let attachment = PromptAttachment::from_clipboard_image(&img).unwrap();
        assert!(attachment.is_image());
        assert_eq!(attachment.label(), "Clipboard (2×2)");
        let caps = AgentCapabilitiesSummary {
            supports_image: true,
            ..Default::default()
        };
        let blocks =
            build_prompt_blocks("check this", &[attachment], caps, &OpenTexts::new()).unwrap();
        assert_eq!(blocks.len(), 2);
        assert!(matches!(blocks[0], ContentBlock::Image(_)));
        assert!(matches!(blocks[1], ContentBlock::Text(_)));
    }
}
