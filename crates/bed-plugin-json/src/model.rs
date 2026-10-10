use jsonc_parser::{ParseOptions, ast::Value, common::Ranged};
use std::{collections::HashMap, ops::Range, sync::Arc};

pub const MAX_SOURCE_BYTES: usize = 128 * 1024 * 1024;
pub const MAX_MODEL_BYTES: usize = 128 * 1024 * 1024;
pub const MAX_DEPTH: usize = 128;
const MAX_PREVIEW_CHARS: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Object(usize),
    Array(usize),
    String,
    Number,
    Boolean,
    Null,
}
impl Kind {
    pub fn container(self) -> bool {
        matches!(self, Self::Object(_) | Self::Array(_))
    }
}

#[derive(Debug)]
pub struct Node {
    pub label: String,
    pub summary: String,
    pub kind: Kind,
    pub depth: usize,
    /// Exclusive end in the preorder node array, permitting cheap subtree skips.
    pub end: usize,
    /// Container identities distinguish duplicate object keys by occurrence.
    pub path: Option<String>,
    pub range: Range<usize>,
}

#[derive(Debug)]
pub struct Tree {
    pub bytes: Arc<[u8]>,
    pub nodes: Vec<Node>,
}

pub fn jsonc_path(path: &str) -> bool {
    path.rsplit(['/', '\\'])
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("jsonc"))
}

pub fn parse(bytes: Arc<[u8]>, jsonc: bool, cancelled: impl Fn() -> bool) -> Result<Tree, String> {
    if bytes.len() > MAX_SOURCE_BYTES {
        return Err("JSON source exceeds the 128 MiB preview limit.".into());
    }
    let source = std::str::from_utf8(&bytes)
        .map_err(|error| format!("JSON source is not UTF-8 at byte {}.", error.valid_up_to()))?;
    preflight(source, &cancelled)?;
    let parsed = jsonc_parser::parse_to_ast(
        source,
        &Default::default(),
        &ParseOptions {
            allow_comments: jsonc,
            allow_trailing_commas: jsonc,
            allow_loose_object_property_names: false,
            allow_missing_commas: false,
            allow_single_quoted_strings: false,
            allow_hexadecimal_numbers: false,
            allow_unary_plus_numbers: false,
        },
    )
    .map_err(|error| error.to_string())?;
    if cancelled() {
        return Err("Cancelled".into());
    }
    let value = parsed
        .value
        .ok_or_else(|| "Expected a JSON value on line 1 column 1.".to_owned())?;
    let mut builder = Builder {
        nodes: Vec::new(),
        used: 0,
        cancelled: &cancelled,
    };
    builder.append(value, "root".into(), "".into(), 0)?;
    Ok(Tree {
        bytes,
        nodes: builder.nodes,
    })
}

fn location_error(source: &str, offset: usize, message: &str) -> String {
    let prefix = &source[..offset];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let column = prefix
        .rsplit('\n')
        .next()
        .unwrap_or_default()
        .chars()
        .count()
        + 1;
    format!("{message} on line {line} column {column}.")
}

/// Check depth and estimate AST allocation before entering the recursive parser.
/// This scanner borrows bytes and skips strings/comments without allocating.
fn preflight(source: &str, cancelled: &impl Fn() -> bool) -> Result<(), String> {
    let bytes = source.as_bytes();
    let mut index = 0;
    let mut depth = 0usize;
    let mut tokens = 0usize;
    let mut next_cancel_check = 0;
    while index < bytes.len() {
        check_cancelled(index, &mut next_cancel_check, cancelled)?;
        match bytes[index] {
            b'"' => {
                tokens += 1;
                index += 1;
                while index < bytes.len() {
                    check_cancelled(index, &mut next_cancel_check, cancelled)?;
                    match bytes[index] {
                        0..=31 => {
                            return Err(location_error(
                                source,
                                index,
                                "Unescaped control character",
                            ));
                        }
                        b'\\' => {
                            index += 1;
                            if bytes.get(index).is_some_and(|byte| *byte < 32) {
                                return Err(location_error(source, index, "Invalid string escape"));
                            }
                            index += usize::from(index < bytes.len());
                        }
                        b'"' => {
                            index += 1;
                            break;
                        }
                        _ => index += 1,
                    }
                }
            }
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                index += 2;
                while index < bytes.len() && !matches!(bytes[index], b'\n' | b'\r') {
                    check_cancelled(index, &mut next_cancel_check, cancelled)?;
                    index += 1;
                }
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index += 2;
                while index < bytes.len() {
                    check_cancelled(index, &mut next_cancel_check, cancelled)?;
                    if bytes[index] == b'*' && bytes.get(index + 1) == Some(&b'/') {
                        index += 2;
                        break;
                    }
                    index += 1;
                }
            }
            b'{' | b'[' => {
                depth += 1;
                tokens += 1;
                if depth > MAX_DEPTH {
                    return Err(location_error(
                        source,
                        index,
                        "JSON nesting exceeds the limit of 128",
                    ));
                }
                index += 1;
            }
            b'}' | b']' => {
                depth = depth.saturating_sub(1);
                index += 1;
            }
            b' ' | b'\t' | b'\n' | b'\r' | b',' | b':' => index += 1,
            0..=31 => return Err(location_error(source, index, "Invalid JSON whitespace")),
            _ => {
                let character = source[index..].chars().next().unwrap();
                if character.is_whitespace() {
                    return Err(location_error(source, index, "Invalid JSON whitespace"));
                }
                tokens += 1;
                index += character.len_utf8();
                while index < bytes.len()
                    && !matches!(
                        bytes[index],
                        0..=32 | b',' | b':' | b'"' | b'{' | b'}' | b'[' | b']' | b'/'
                    )
                {
                    check_cancelled(index, &mut next_cancel_check, cancelled)?;
                    let character = source[index..].chars().next().unwrap();
                    if character.is_whitespace() {
                        return Err(location_error(source, index, "Invalid JSON whitespace"));
                    }
                    index += character.len_utf8();
                }
            }
        }
        // The AST and flattened rows coexist during conversion. Account for
        // their vectors and escaped strings conservatively before parsing.
        if tokens.saturating_mul(512).saturating_add(bytes.len()) > MAX_MODEL_BYTES {
            return Err("JSON structure exceeds the 128 MiB preview model limit.".into());
        }
    }
    Ok(())
}

fn check_cancelled(
    index: usize,
    next_check: &mut usize,
    cancelled: &impl Fn() -> bool,
) -> Result<(), String> {
    if index >= *next_check {
        if cancelled() {
            return Err("Cancelled".into());
        }
        *next_check = index + 16 * 1024;
    }
    Ok(())
}

pub fn preview(value: &str) -> String {
    let mut characters = value.chars();
    let mut result: String = characters.by_ref().take(MAX_PREVIEW_CHARS).collect();
    if characters.next().is_some() {
        result.push('…');
    }
    result
}

fn quoted_preview(value: &str) -> String {
    serde_json::to_string(&preview(value)).expect("strings serialize as JSON")
}

struct Builder<'a, F> {
    nodes: Vec<Node>,
    used: usize,
    cancelled: &'a F,
}
impl<F: Fn() -> bool> Builder<'_, F> {
    fn spend(&mut self, bytes: usize) -> Result<(), String> {
        self.used = self.used.saturating_add(bytes);
        if self.used > MAX_MODEL_BYTES {
            Err("JSON structure exceeds the 128 MiB preview model limit.".into())
        } else {
            Ok(())
        }
    }
    fn child_path(&mut self, parent: &str, segment: &str) -> Result<String, String> {
        self.spend(parent.len().saturating_add(segment.len()).saturating_add(1))?;
        Ok(format!("{parent}/{segment}"))
    }
    fn append(
        &mut self,
        value: Value<'_>,
        label: String,
        path: String,
        depth: usize,
    ) -> Result<(), String> {
        if (self.cancelled)() {
            return Err("Cancelled".into());
        }
        let range = value.range();
        let (kind, summary) = match &value {
            Value::Object(object) => (
                Kind::Object(object.properties.len()),
                format!("{{}}  {} properties", object.properties.len()),
            ),
            Value::Array(array) => (
                Kind::Array(array.elements.len()),
                format!("[]  {} items", array.elements.len()),
            ),
            Value::StringLit(string) => (Kind::String, quoted_preview(&string.value)),
            Value::NumberLit(number) => (Kind::Number, preview(number.value)),
            Value::BooleanLit(boolean) => (Kind::Boolean, boolean.value.to_string()),
            Value::NullKeyword(_) => (Kind::Null, "null".into()),
        };
        self.spend(std::mem::size_of::<Node>() * 2 + label.len() + summary.len())?;
        let index = self.nodes.len();
        self.nodes.push(Node {
            label,
            summary,
            kind,
            depth,
            end: index + 1,
            path: kind.container().then(|| path.clone()),
            range: range.start..range.end,
        });
        match value {
            Value::Object(object) => {
                let mut occurrences = HashMap::<String, usize>::new();
                for property in object.properties {
                    let name = property.name.as_str();
                    let occurrence = *occurrences.get(name).unwrap_or(&0);
                    if occurrence == 0 {
                        self.spend(name.len().saturating_add(128))?;
                    }
                    let child_path = if matches!(property.value, Value::Object(_) | Value::Array(_))
                    {
                        self.spend(name.len().saturating_mul(2).saturating_add(32))?;
                        let escaped = name.replace('~', "~0").replace('/', "~1");
                        self.child_path(&path, &format!("p:{escaped}:{occurrence}"))?
                    } else {
                        String::new()
                    };
                    let label = quoted_preview(name);
                    let count = occurrences.entry(property.name.into_string()).or_default();
                    *count += 1;
                    self.append(property.value, label, child_path, depth + 1)?;
                }
            }
            Value::Array(array) => {
                for (position, element) in array.elements.into_iter().enumerate() {
                    let child_path = if matches!(element, Value::Object(_) | Value::Array(_)) {
                        self.child_path(&path, &format!("i:{position}"))?
                    } else {
                        String::new()
                    };
                    self.append(element, format!("[{position}]"), child_path, depth + 1)?;
                }
            }
            _ => {}
        }
        self.nodes[index].end = self.nodes.len();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(source: &str, jsonc: bool) -> Result<Tree, String> {
        parse(Arc::from(source.as_bytes()), jsonc, || false)
    }

    #[test]
    fn preserves_property_order_duplicate_keys_and_number_lexemes() {
        let source = r#"{"z": 123456789012345678901234567890, "a": -0.000E+9999, "z": []}"#;
        let tree = tree(source, false).unwrap();
        assert_eq!(tree.nodes.len(), 4);
        assert_eq!(tree.nodes[1].label, "\"z\"");
        assert_eq!(tree.nodes[2].label, "\"a\"");
        assert_eq!(tree.nodes[3].label, "\"z\"");
        assert_eq!(tree.nodes[1].summary, "123456789012345678901234567890");
        assert_eq!(tree.nodes[2].summary, "-0.000E+9999");
        assert_eq!(tree.nodes[3].path.as_deref(), Some("/p:z:1"));
    }

    #[test]
    fn jsonc_enables_only_comments_and_trailing_commas() {
        for source in ["[1,]", "{/* comment */\"x\": true,}", "// comment\nnull"] {
            assert!(tree(source, true).is_ok(), "{source}");
            assert!(tree(source, false).is_err(), "{source}");
        }
        for source in [
            "{key:1}",
            "{'key':1}",
            "[1 2]",
            "{\"a\":1 \"b\":2}",
            "+1",
            "0x01",
            "01",
            "1.",
            "[true, false] garbage",
            "[1,\u{a0}2]",
            "\"raw\nnewline\"",
        ] {
            assert!(tree(source, true).is_err(), "{source}");
            assert!(tree(source, false).is_err(), "{source}");
        }
    }

    #[test]
    fn unicode_scalars_empty_containers_and_error_locations() {
        for source in ["null", "true", "123", "\"🍋\\uD83D\\uDE80\"", "[]", "{}"] {
            assert!(tree(source, false).is_ok(), "{source}");
        }
        assert_eq!(
            tree("\"\\uD83D\\uDE80\"", false).unwrap().nodes[0].summary,
            "\"🚀\""
        );
        assert!(
            tree("{\n  \"🍋\":\n}", false)
                .unwrap_err()
                .contains("line 3 column 1")
        );
        assert!(tree("", false).unwrap_err().contains("line 1 column 1"));
        assert!(parse(Arc::from(&b"\xff"[..]), false, || false).is_err());
    }

    #[test]
    fn checks_depth_before_recursive_parse_while_ignoring_strings_and_comments() {
        let allowed = format!("{}0{}", "[".repeat(MAX_DEPTH), "]".repeat(MAX_DEPTH));
        assert!(tree(&allowed, false).is_ok());
        let too_deep = format!("{}0{}", "[".repeat(30_000), "]".repeat(30_000));
        assert!(tree(&too_deep, false).unwrap_err().contains("limit of 128"));
        let quoted = serde_json::to_string(&"[".repeat(30_000)).unwrap();
        assert!(tree(&quoted, false).is_ok());
        assert!(tree(&format!("/*{}*/ null", "[".repeat(30_000)), true).is_ok());
        assert!(
            parse(Arc::from(allowed.as_bytes()), false, || true)
                .unwrap_err()
                .contains("Cancelled")
        );
    }

    #[test]
    fn bounds_preview_models_and_keeps_paths_unambiguous() {
        let dense = format!("[{}0]", "0,".repeat(MAX_MODEL_BYTES / 256));
        assert!(tree(&dense, false).unwrap_err().contains("model limit"));
        let tree = tree(r#"{"a/b": [], "a~1b": [], "a/b": {"x":[]}}"#, false).unwrap();
        assert_eq!(tree.nodes[1].path.as_deref(), Some("/p:a~1b:0"));
        assert_eq!(tree.nodes[2].path.as_deref(), Some("/p:a~01b:0"));
        assert_eq!(tree.nodes[3].path.as_deref(), Some("/p:a~1b:1"));
        assert_eq!(tree.nodes[4].path.as_deref(), Some("/p:a~1b:1/p:x:0"));
        assert_eq!(tree.nodes[0].end, 5);
    }
}
