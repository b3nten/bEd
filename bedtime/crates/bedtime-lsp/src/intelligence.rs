//! Worker-side validation of hover and completion payloads. Byte ranges refer
//! exclusively to the captured source root, never the current screen layout.
use crate::*;
use bedtime_editor::{CompletionEdit, TextEdit};
use serde_json::{Value, json};
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum QueryKind {
    Hover,
    Completion,
    Resolve,
}
#[derive(Clone, Debug)]
pub enum LanguageQuery {
    Hover { byte: usize },
    Completion { byte: usize, context: Value },
    Resolve { byte: usize, item: CompletionData },
}
impl LanguageQuery {
    pub fn kind(&self) -> QueryKind {
        match self {
            Self::Hover { .. } => QueryKind::Hover,
            Self::Completion { .. } => QueryKind::Completion,
            Self::Resolve { .. } => QueryKind::Resolve,
        }
    }
    pub fn byte(&self) -> usize {
        match self {
            Self::Hover { byte } | Self::Completion { byte, .. } | Self::Resolve { byte, .. } => {
                *byte
            }
        }
    }
}
#[derive(Clone, Debug, Default)]
pub struct Markup {
    pub text: String,
    pub markdown: bool,
}
#[derive(Clone, Debug)]
pub struct Hover {
    pub contents: Markup,
    pub range: Option<Range<usize>>,
}
#[derive(Clone, Debug)]
pub struct CompletionData {
    pub(crate) value: Arc<Value>,
    pub(crate) bytes: usize,
}
#[derive(Clone, Debug)]
pub struct CompletionEntry {
    pub label: String,
    pub detail: String,
    pub documentation: Markup,
    pub filter: String,
    pub sort: String,
    pub edit: CompletionEdit,
    /// Unmodified server data, retained for completionItem/resolve.
    pub raw: CompletionData,
}
#[derive(Clone, Debug)]
pub struct Completions {
    pub incomplete: bool,
    pub items: Vec<CompletionEntry>,
    pub choices: Arc<[bedtime_editor::CompletionChoice]>,
}
#[derive(Debug)]
pub enum Intelligence {
    Hover(Option<Hover>),
    Completion(Completions),
    Resolved(CompletionEntry),
}

fn text(value: &Value, limit: usize) -> Result<String, String> {
    let s = value
        .as_str()
        .ok_or("Expected a string in language response")?;
    if s.len() > limit {
        return Err("Language response text exceeds limit".into());
    }
    Ok(s.to_owned())
}
pub(crate) fn markup(value: &Value) -> Result<Markup, String> {
    if value.is_null() {
        return Ok(Markup::default());
    }
    if value.is_string() {
        return Ok(Markup {
            text: text(value, 64 * 1024)?,
            markdown: false,
        });
    }
    if let Some(values) = value.as_array() {
        let mut result = Markup {
            text: String::new(),
            markdown: true,
        };
        for value in values.iter().take(64) {
            let part = markup(value)?;
            if !result.text.is_empty() {
                result.text.push_str("\n\n");
            }
            result.text.push_str(&part.text);
            if result.text.len() > 64 * 1024 {
                return Err("Hover exceeds 64 KiB".into());
            }
        }
        return Ok(result);
    }
    let body = text(&value["value"], 64 * 1024)?;
    if let Some(language) = value["language"].as_str() {
        return Ok(Markup {
            text: format!(
                "```{}\n{}\n```",
                language.chars().take(64).collect::<String>(),
                body
            ),
            markdown: true,
        });
    }
    Ok(Markup {
        text: body,
        markdown: value["kind"] == "markdown",
    })
}
pub(crate) fn parse_hover(
    value: &Value,
    source: &TextBuffer,
    encoding: Encoding,
) -> Result<Option<Hover>, String> {
    if value.is_null() {
        return Ok(None);
    }
    let mut contents = markup(&value["contents"])?;
    // Legacy Hover.contents strings are Markdown MarkedStrings.
    if value["contents"].is_string() {
        contents.markdown = true;
    }
    let range = value
        .get("range")
        .filter(|v| !v.is_null())
        .map(|v| {
            encoding.range(
                source,
                serde_json::from_value(v.clone()).map_err(|_| "Invalid hover range")?,
            )
        })
        .transpose()?;
    Ok((!contents.text.trim().is_empty()).then_some(Hover { contents, range }))
}
pub(crate) fn parse_completions(
    value: &Value,
    source: &TextBuffer,
    byte: usize,
    encoding: Encoding,
) -> Result<Completions, String> {
    if value.is_null() {
        return Ok(Completions {
            incomplete: false,
            items: vec![],
            choices: Arc::from([]),
        });
    }
    let values = value
        .as_array()
        .or_else(|| value["items"].as_array())
        .ok_or("Invalid completion list")?;
    if values.len() > 10000 {
        return Err("Completion list exceeds 10,000 items".into());
    }
    let mut items = Vec::with_capacity(values.len());
    let mut prefixes = std::collections::HashMap::new();
    for value in values {
        // Reject an unusable item without discarding other valid suggestions.
        if let Ok(item) = parse_completion(value, source, byte, encoding) {
            if byte - item.edit.primary.range.start > 16384 {
                continue;
            }
            let prefix = prefixes
                .entry(item.edit.primary.range.start)
                .or_insert_with(|| {
                    source
                        .slice(item.edit.primary.range.start..byte)
                        .to_string()
                        .to_lowercase()
                });
            let filter = item.filter.to_lowercase();
            let mut matching = prefix.chars();
            let mut wanted = matching.next();
            for ch in filter.chars() {
                if wanted == Some(ch) {
                    wanted = matching.next();
                }
            }
            if wanted.is_none() {
                items.push(item);
            }
        }
    }
    items.sort_by(|a, b| a.sort.cmp(&b.sort).then(a.label.cmp(&b.label)));
    let choices = items
        .iter()
        .map(|i| bedtime_editor::CompletionChoice {
            kind: match i.raw.value["kind"].as_u64().unwrap_or(0) {
                2 | 3 => "fn",
                4 => "ctor",
                5 => "field",
                6 => "var",
                7 => "type",
                8 => "trait",
                9 => "mod",
                10 => "prop",
                11 => "unit",
                12 => "value",
                13 | 20 => "enum",
                14 => "kw",
                17 => "file",
                18 => "ref",
                19 => "dir",
                21 => "const",
                22 => "struct",
                23 => "event",
                24 => "op",
                25 => "T",
                _ => "·",
            }
            .into(),
            label: i.label.as_str().into(),
            detail: {
                let detail = i.raw.value["labelDetails"]["detail"].as_str().unwrap_or("");
                let description = i.raw.value["labelDetails"]["description"]
                    .as_str()
                    .unwrap_or("");
                [detail, description]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(" · ")
                    .into()
            },
        })
        .collect::<Vec<_>>()
        .into();
    Ok(Completions {
        incomplete: value["isIncomplete"] == true,
        items,
        choices,
    })
}
fn identifier_start(source: &TextBuffer, byte: usize) -> Result<usize, String> {
    if !source.is_char_boundary(byte) {
        return Err("Invalid completion position".into());
    }
    let mut start = byte;
    while start > 0 {
        let (chunk, base) = source.chunk_at(start - 1);
        for ch in chunk[..start - base].chars().rev() {
            if !ch.is_alphanumeric() && ch != '_' {
                return Ok(start);
            }
            start -= ch.len_utf8();
            if byte - start > 16384 {
                return Err("Completion prefix exceeds 16 KiB".into());
            }
        }
    }
    Ok(start)
}
pub(crate) fn parse_completion(
    value: &Value,
    source: &TextBuffer,
    byte: usize,
    encoding: Encoding,
) -> Result<CompletionEntry, String> {
    let label = text(&value["label"], 4096)?;
    let start = identifier_start(source, byte)?;
    let mut range = start..byte;
    let new_text = if let Some(edit) = value.get("textEdit").filter(|v| !v.is_null()) {
        let raw_range = edit
            .get("range")
            .or_else(|| edit.get("replace"))
            .ok_or("Missing completion edit range")?;
        let protocol: ProtocolRange = serde_json::from_value(raw_range.clone())
            .map_err(|_| "Invalid completion edit range")?;
        if protocol.start.line != protocol.end.line
            || protocol.start.line as usize != source.line_of(byte)
        {
            return Err("Completion replacement must be on the request line".into());
        }
        range = encoding.range(source, protocol)?;
        if let Some(insert) = edit.get("insert") {
            let insert = encoding.range(
                source,
                serde_json::from_value(insert.clone())
                    .map_err(|_| "Invalid completion insert range")?,
            )?;
            if insert.start != range.start
                || insert.end > range.end
                || !(insert.start <= byte && byte <= insert.end)
            {
                return Err("Invalid insert/replace completion ranges".into());
            }
        }
        if !(range.start <= byte && byte <= range.end) {
            return Err("Completion edit does not contain the request position".into());
        }
        text(&edit["newText"], MAX_SOURCE_BYTES)?
    } else {
        text(
            value.get("insertText").unwrap_or(&value["label"]),
            MAX_SOURCE_BYTES,
        )?
    };
    let mut additional = Vec::new();
    if let Some(edits) = value.get("additionalTextEdits").filter(|v| !v.is_null()) {
        let edits = edits
            .as_array()
            .ok_or("Invalid additional completion edits")?;
        if edits.len() > 128 {
            return Err("Too many additional completion edits".into());
        }
        for edit in edits {
            additional.push(TextEdit {
                range: encoding.range(
                    source,
                    serde_json::from_value(edit["range"].clone())
                        .map_err(|_| "Invalid additional edit range")?,
                )?,
                replacement: text(&edit["newText"], MAX_SOURCE_BYTES)?,
            });
        }
    }
    let edit = CompletionEdit {
        primary: TextEdit {
            range,
            replacement: new_text,
        },
        additional,
        snippet: value["insertTextFormat"] == 2,
    };
    // Validate overlapping ranges and snippet syntax before advertising the item.
    edit.prepare(source)?;
    Ok(CompletionEntry {
        detail: value
            .get("detail")
            .filter(|v| !v.is_null())
            .map(|v| text(v, 16384))
            .transpose()?
            .unwrap_or_default(),
        documentation: markup(&value["documentation"])?,
        filter: value["filterText"].as_str().unwrap_or(&label).to_owned(),
        sort: value["sortText"].as_str().unwrap_or(&label).to_owned(),
        label,
        edit,
        raw: CompletionData {
            value: Arc::new(value.clone()),
            bytes: value.to_string().len(),
        },
    })
}
/// LSP completion contexts: invoked, trigger character, incomplete refresh.
pub fn completion_context(trigger: Option<&str>, incomplete: bool) -> Value {
    if incomplete {
        json!({"triggerKind":3})
    } else if let Some(trigger) = trigger {
        json!({"triggerKind":2,"triggerCharacter":trigger})
    } else {
        json!({"triggerKind":1})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn completion_ranges_follow_negotiated_encoding_and_reject_overlap() {
        let source = TextBuffer::from("🙂foo");
        let value = json!({"label":"food","textEdit":{"insert":{"start":{"line":0,"character":2},"end":{"line":0,"character":4}},"replace":{"start":{"line":0,"character":2},"end":{"line":0,"character":5}},"newText":"food"}});
        let item = parse_completion(&value, &source, 6, Encoding::Utf16).unwrap();
        assert_eq!(item.edit.primary.range, 4..7);
        let mut invalid = value.clone();
        invalid["additionalTextEdits"] = json!([{"range":{"start":{"line":0,"character":3},"end":{"line":0,"character":4}},"newText":"x"}]);
        assert!(parse_completion(&invalid, &source, 6, Encoding::Utf16).is_err());
        let mut surrogate = value;
        surrogate["textEdit"]["replace"]["start"]["character"] = json!(1);
        assert!(parse_completion(&surrogate, &source, 6, Encoding::Utf16).is_err());
    }
    #[test]
    fn malformed_completion_does_not_hide_usable_items_or_fail_the_server() {
        let list = json!({"isIncomplete":true,"items":[{"label":"good","insertText":"good"},{"label":"bad","insertText":"${1/a/b/}","insertTextFormat":2}]});
        let result = parse_completions(&list, &TextBuffer::from("g"), 1, Encoding::Utf8).unwrap();
        assert!(result.incomplete);
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].label, "good");
    }
    #[test]
    fn hover_accepts_legacy_content_and_checks_optional_range() {
        let value = json!({"contents":[{"language":"go","value":"func Hello()"},"Documentation"],"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":5}}});
        let hover = parse_hover(&value, &TextBuffer::from("Hello"), Encoding::Utf8)
            .unwrap()
            .unwrap();
        assert!(hover.contents.markdown);
        assert!(hover.contents.text.contains("```go"));
        assert_eq!(hover.range, Some(0..5));
        assert!(
            parse_hover(&Value::Null, &TextBuffer::default(), Encoding::Utf8)
                .unwrap()
                .is_none()
        );
    }
}
