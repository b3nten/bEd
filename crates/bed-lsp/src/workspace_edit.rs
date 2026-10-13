//! Validation of server edits at the protocol boundary. Positions remain UTF-16
//! until they are converted against the exact document snapshot being edited.
use crate::lsp_uri::LspUri;
use serde_json::Value;
use std::{collections::BTreeMap, io, ops::Range, path::PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Position {
    pub line: usize,
    pub character: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextEdit {
    pub start: Position,
    pub end: Position,
    pub new_text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentEdit {
    pub path: PathBuf,
    pub version: Option<i32>,
    pub edits: Vec<TextEdit>,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn position(value: &Value) -> io::Result<Position> {
    let number = |key| {
        value
            .get(key)
            .and_then(Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| invalid("Expected a nonnegative LSP position"))
    };
    Ok(Position {
        line: number("line")?,
        character: number("character")?,
    })
}

pub fn parse_text_edits(value: &Value) -> io::Result<Vec<TextEdit>> {
    if value.is_null() {
        return Ok(Vec::new());
    }
    value
        .as_array()
        .ok_or_else(|| invalid("Expected LSP text edit array"))?
        .iter()
        .map(|edit| {
            let start = position(&edit["range"]["start"])?;
            let end = position(&edit["range"]["end"])?;
            if start > end {
                return Err(invalid("LSP edit range is reversed"));
            }
            let new_text = edit["newText"]
                .as_str()
                .ok_or_else(|| invalid("Expected LSP replacement text"))?
                .to_owned();
            Ok(TextEdit {
                start,
                end,
                new_text,
            })
        })
        .collect()
}

fn path(uri: &str) -> io::Result<PathBuf> {
    let uri = LspUri::parse(uri)?;
    if !uri.is_file_uri()
        || !uri.authority().is_empty()
        || uri.has_query()
        || uri.has_fragment()
        || !uri.path().starts_with('/')
        || uri.path().contains('\0')
    {
        return Err(invalid("Workspace edits require absolute file URIs"));
    }
    Ok(PathBuf::from(uri.fs_path()))
}

pub fn parse_workspace_edit(value: &Value) -> io::Result<Vec<DocumentEdit>> {
    if value.is_null() {
        return Ok(Vec::new());
    }
    if !value.is_object() {
        return Err(invalid("Expected LSP WorkspaceEdit"));
    }
    if value.get("changes").is_some() && value.get("documentChanges").is_some() {
        return Err(invalid(
            "WorkspaceEdit cannot combine changes and documentChanges",
        ));
    }
    if value["changeAnnotations"]
        .as_object()
        .is_some_and(|annotations| annotations.values().any(|a| a["needsConfirmation"] == true))
    {
        return Err(invalid(
            "Workspace edits requiring confirmation are unsupported",
        ));
    }
    let mut documents = BTreeMap::new();
    if let Some(changes) = value.get("changes") {
        for (uri, edits) in changes
            .as_object()
            .ok_or_else(|| invalid("Expected workspace changes object"))?
        {
            let path = path(uri)?;
            documents.insert(
                path.clone(),
                DocumentEdit {
                    path,
                    version: None,
                    edits: parse_text_edits(edits)?,
                },
            );
        }
    }
    if let Some(changes) = value.get("documentChanges") {
        for edit in changes
            .as_array()
            .ok_or_else(|| invalid("Expected documentChanges array"))?
        {
            if edit.get("kind").is_some() {
                return Err(invalid(
                    "LSP file create, rename and delete operations are unsupported",
                ));
            }
            let document = &edit["textDocument"];
            let path = path(
                document["uri"]
                    .as_str()
                    .ok_or_else(|| invalid("Expected edited document URI"))?,
            )?;
            let version = match document.get("version") {
                None | Some(Value::Null) => None,
                Some(value) => Some(
                    value
                        .as_i64()
                        .and_then(|n| i32::try_from(n).ok())
                        .ok_or_else(|| invalid("Expected document version"))?,
                ),
            };
            if documents.contains_key(&path) {
                return Err(invalid("WorkspaceEdit repeats a document"));
            }
            documents.insert(
                path.clone(),
                DocumentEdit {
                    path,
                    version,
                    edits: parse_text_edits(&edit["edits"])?,
                },
            );
        }
    }
    Ok(documents.into_values().collect())
}

pub fn position_offset(bytes: &[u8], position: Position) -> io::Result<usize> {
    let text = std::str::from_utf8(bytes).map_err(|_| invalid("LSP edits require UTF-8 text"))?;
    let mut start = 0;
    for _ in 0..position.line {
        let end = start
            + text[start..]
                .find(['\r', '\n'])
                .ok_or_else(|| invalid("LSP position is beyond document"))?;
        start = end
            + if text.as_bytes().get(end..end + 2) == Some(b"\r\n") {
                2
            } else {
                1
            };
    }
    let end = text[start..]
        .find(['\r', '\n'])
        .map_or(text.len(), |offset| start + offset);
    let line = &text[start..end];
    let mut utf16 = 0;
    for (offset, character) in line.char_indices() {
        if utf16 == position.character {
            return Ok(start + offset);
        }
        utf16 += character.len_utf16();
        if utf16 > position.character {
            return Err(invalid("LSP position splits a UTF-16 surrogate pair"));
        }
    }
    if utf16 == position.character {
        Ok(start + line.len())
    } else {
        Err(invalid("LSP position is beyond line"))
    }
}

pub fn byte_edits(bytes: &[u8], edits: &[TextEdit]) -> io::Result<Vec<(Range<usize>, Vec<u8>)>> {
    let mut result = edits
        .iter()
        .map(|edit| {
            Ok((
                position_offset(bytes, edit.start)?..position_offset(bytes, edit.end)?,
                edit.new_text.as_bytes().to_vec(),
            ))
        })
        .collect::<io::Result<Vec<_>>>()?;
    result.sort_by_key(|(range, _)| (range.start, range.end));
    for pair in result.windows(2) {
        if pair[1].0.start < pair[0].0.end || pair[1].0.start == pair[0].0.start {
            return Err(invalid("LSP edit ranges overlap"));
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn utf16_positions_validate_surrogates_crlf_and_trailing_line() {
        let text = "é🙂x\r\nnext\n".as_bytes();
        assert_eq!(
            position_offset(
                text,
                Position {
                    line: 0,
                    character: 3
                }
            )
            .unwrap(),
            6
        );
        assert!(
            position_offset(
                text,
                Position {
                    line: 0,
                    character: 2
                }
            )
            .is_err()
        );
        assert!(
            position_offset(
                text,
                Position {
                    line: 0,
                    character: 5
                }
            )
            .is_err()
        );
        assert_eq!(
            position_offset(
                text,
                Position {
                    line: 2,
                    character: 0
                }
            )
            .unwrap(),
            text.len()
        );
        assert_eq!(
            position_offset(
                b"one\rtwo\r",
                Position {
                    line: 1,
                    character: 3
                }
            )
            .unwrap(),
            7
        );
    }

    #[test]
    fn workspace_boundary_rejects_resource_operations_and_bad_versions() {
        assert!(parse_workspace_edit(&json!({"documentChanges":[{"kind":"rename","oldUri":"file:///a","newUri":"file:///b"}]})).is_err());
        assert!(parse_workspace_edit(&json!({"documentChanges":[{"textDocument":{"uri":"file:///a","version":1.5},"edits":[]}]})).is_err());
        let edits = parse_workspace_edit(&json!({"changes":{"file:///tmp/a%20b":[]}})).unwrap();
        assert_eq!(edits[0].path, PathBuf::from("/tmp/a b"));
        assert!(parse_text_edits(&Value::Null).unwrap().is_empty());
    }
}
