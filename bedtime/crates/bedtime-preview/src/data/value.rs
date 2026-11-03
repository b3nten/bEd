//! JSON and TOML as one tree: scalars, arrays and tables (objects), keys in
//! the file's order. Both parse through serde into `Value`.
use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use std::fmt;

/// The key `toml` gives a datetime, read as a map.
const TOML_DATETIME: &str = "$__toml_private_datetime";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Json,
    Toml,
}

impl Format {
    /// By file name: `.json`, `.toml`, and lock files that are TOML.
    pub fn of(name: &str) -> Option<Self> {
        let name = name.to_ascii_lowercase();
        let extension = name.rsplit_once('.').map(|(_, extension)| extension);
        match extension {
            Some("json" | "geojson" | "webmanifest") => Some(Self::Json),
            Some("toml") => Some(Self::Toml),
            _ if matches!(name.as_str(), "cargo.lock" | "uv.lock" | "poetry.lock") => {
                Some(Self::Toml)
            }
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    String,
    Number,
    Bool,
    Null,
    Date,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Scalar(Kind, String),
    Array(Vec<Value>),
    Table(Vec<(String, Value)>),
}

impl Value {
    /// Entries of an array or table, keyed by index or name.
    pub fn children(&self) -> Option<Vec<(Key<'_>, &Value)>> {
        match self {
            Value::Scalar(..) => None,
            Value::Array(items) => Some(
                items
                    .iter()
                    .enumerate()
                    .map(|(i, v)| (Key::Index(i), v))
                    .collect(),
            ),
            Value::Table(entries) => Some(entries.iter().map(|(k, v)| (Key::Name(k), v)).collect()),
        }
    }

    pub fn child(&self, key: &Key<'_>) -> Option<&Value> {
        match (self, key) {
            (Value::Array(items), Key::Index(i)) => items.get(*i),
            (Value::Table(entries), Key::Name(name)) => {
                entries.iter().find(|(k, _)| k == name).map(|(_, v)| v)
            }
            _ => None,
        }
    }

    /// "3 keys", "12 items".
    pub fn summary(&self) -> String {
        let (n, one, many) = match self {
            Value::Scalar(_, text) => return text.clone(),
            Value::Array(items) => (items.len(), "item", "items"),
            Value::Table(entries) => (entries.len(), "key", "keys"),
        };
        format!("{n} {}", if n == 1 { one } else { many })
    }

    /// As JSON: a scalar's text alone, containers indented.
    pub fn to_json(&self) -> String {
        let mut out = String::new();
        match self {
            Value::Scalar(Kind::String | Kind::Date, text) => out.push_str(text),
            _ => write_json(self, 0, &mut out),
        }
        out
    }
}

fn write_json(value: &Value, indent: usize, out: &mut String) {
    let pad = |n: usize, out: &mut String| out.extend(std::iter::repeat_n("  ", n));
    match value {
        Value::Scalar(Kind::String | Kind::Date, text) => {
            out.push_str(&serde_json::Value::String(text.clone()).to_string())
        }
        Value::Scalar(_, text) => out.push_str(text),
        Value::Array(items) if items.is_empty() => out.push_str("[]"),
        Value::Table(entries) if entries.is_empty() => out.push_str("{}"),
        Value::Array(items) => {
            out.push_str("[\n");
            for (i, item) in items.iter().enumerate() {
                pad(indent + 1, out);
                write_json(item, indent + 1, out);
                out.push_str(if i + 1 < items.len() { ",\n" } else { "\n" });
            }
            pad(indent, out);
            out.push(']');
        }
        Value::Table(entries) => {
            out.push_str("{\n");
            for (i, (key, item)) in entries.iter().enumerate() {
                pad(indent + 1, out);
                out.push_str(&serde_json::Value::String(key.clone()).to_string());
                out.push_str(": ");
                write_json(item, indent + 1, out);
                out.push_str(if i + 1 < entries.len() { ",\n" } else { "\n" });
            }
            pad(indent, out);
            out.push('}');
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key<'a> {
    Index(usize),
    Name(&'a str),
}

/// Where a value is, as written in jq or JavaScript: `package.authors[0]`,
/// `["key with spaces"]`.
pub fn path_text(keys: &[Key<'_>]) -> String {
    let mut out = String::new();
    for key in keys {
        match key {
            Key::Index(i) => out.push_str(&format!("[{i}]")),
            Key::Name(name) if is_identifier(name) => {
                if !out.is_empty() {
                    out.push('.');
                }
                out.push_str(name);
            }
            Key::Name(name) => {
                out.push('[');
                out.push_str(&serde_json::Value::String((*name).into()).to_string());
                out.push(']');
            }
        }
    }
    out
}

fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_alphanumeric() || c == '_' || c == '$' || c == '-')
}

/// Parse `text`; an error says where (line and column from 1).
pub fn parse(format: Format, text: &str) -> Result<Value, String> {
    match format {
        Format::Json => serde_json::from_str(text).map_err(|error| error.to_string()),
        Format::Toml => toml::from_str(text).map_err(|error| {
            let message = error.message().trim_end();
            match error.span() {
                Some(span) => {
                    let (line, column) = line_column(text, span.start);
                    format!("{message} at line {line} column {column}")
                }
                None => message.to_owned(),
            }
        }),
    }
}

fn line_column(text: &str, byte: usize) -> (usize, usize) {
    let before = &text[..byte.min(text.len())];
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().unwrap_or("").chars().count() + 1;
    (line, column)
}

impl<'de> Deserialize<'de> for Value {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(ValueVisitor)
    }
}

struct ValueVisitor;

impl<'de> Visitor<'de> for ValueVisitor {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a JSON or TOML value")
    }
    fn visit_bool<E>(self, v: bool) -> Result<Value, E> {
        Ok(Value::Scalar(Kind::Bool, v.to_string()))
    }
    fn visit_i64<E>(self, v: i64) -> Result<Value, E> {
        Ok(Value::Scalar(Kind::Number, v.to_string()))
    }
    fn visit_u64<E>(self, v: u64) -> Result<Value, E> {
        Ok(Value::Scalar(Kind::Number, v.to_string()))
    }
    fn visit_f64<E>(self, v: f64) -> Result<Value, E> {
        Ok(Value::Scalar(Kind::Number, format!("{v:?}")))
    }
    fn visit_str<E>(self, v: &str) -> Result<Value, E> {
        Ok(Value::Scalar(Kind::String, v.into()))
    }
    fn visit_string<E>(self, v: String) -> Result<Value, E> {
        Ok(Value::Scalar(Kind::String, v))
    }
    fn visit_unit<E>(self) -> Result<Value, E> {
        Ok(Value::Scalar(Kind::Null, "null".into()))
    }
    fn visit_none<E: serde::de::Error>(self) -> Result<Value, E> {
        self.visit_unit()
    }
    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Value, D::Error> {
        Value::deserialize(d)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut items = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        while let Some(item) = seq.next_element()? {
            items.push(item);
        }
        Ok(Value::Array(items))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut entries = Vec::with_capacity(map.size_hint().unwrap_or(0));
        while let Some(key) = map.next_key::<String>()? {
            if key == TOML_DATETIME && entries.is_empty() {
                let date: String = map.next_value()?;
                return Ok(Value::Scalar(Kind::Date, date));
            }
            entries.push((key, map.next_value()?));
        }
        Ok(Value::Table(entries))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(value: &Value) -> Vec<&str> {
        match value {
            Value::Table(entries) => entries.iter().map(|(k, _)| k.as_str()).collect(),
            _ => panic!("not a table"),
        }
    }

    #[test]
    fn json_keeps_the_files_order_and_kinds() {
        let value = parse(
            Format::Json,
            r#"{"z": 1, "a": [true, null, 2.5, "x"], "m": {}}"#,
        )
        .unwrap();
        assert_eq!(keys(&value), ["z", "a", "m"]);
        let a = value.child(&Key::Name("a")).unwrap();
        let kinds: Vec<Kind> = a
            .children()
            .unwrap()
            .into_iter()
            .map(|(_, v)| match v {
                Value::Scalar(kind, _) => *kind,
                _ => panic!(),
            })
            .collect();
        assert_eq!(kinds, [Kind::Bool, Kind::Null, Kind::Number, Kind::String]);
        assert_eq!(a.summary(), "4 items");
    }

    #[test]
    fn toml_keeps_order_and_reads_dates() {
        let text =
            "title = \"x\"\nwhen = 1979-05-27T07:32:00Z\n\n[[bin]]\nname = \"b\"\n\n[a]\nk = 1\n";
        let value = parse(Format::Toml, text).unwrap();
        assert_eq!(keys(&value), ["title", "when", "bin", "a"]);
        assert_eq!(
            value.child(&Key::Name("when")),
            Some(&Value::Scalar(Kind::Date, "1979-05-27T07:32:00Z".into()))
        );
        assert!(matches!(
            value.child(&Key::Name("bin")),
            Some(Value::Array(_))
        ));
    }

    #[test]
    fn errors_say_where() {
        let error = parse(Format::Toml, "a = 1\nb = \n").unwrap_err();
        assert!(error.contains("line 2"), "{error}");
        let error = parse(Format::Json, "{\n  \"a\": }").unwrap_err();
        assert!(error.contains("line 2"), "{error}");
    }

    #[test]
    fn paths_and_copies() {
        assert_eq!(
            path_text(&[Key::Name("package"), Key::Name("authors"), Key::Index(0)]),
            "package.authors[0]"
        );
        assert_eq!(path_text(&[Key::Name("a b")]), "[\"a b\"]");
        let value = parse(Format::Json, r#"{"a": [1, "two"]}"#).unwrap();
        assert_eq!(
            value.to_json(),
            "{\n  \"a\": [\n    1,\n    \"two\"\n  ]\n}"
        );
        assert_eq!(Value::Scalar(Kind::String, "hi".into()).to_json(), "hi");
    }

    #[test]
    fn formats_by_name() {
        assert_eq!(Format::of("package.json"), Some(Format::Json));
        assert_eq!(Format::of("Cargo.toml"), Some(Format::Toml));
        assert_eq!(Format::of("Cargo.lock"), Some(Format::Toml));
        assert_eq!(Format::of("notes.md"), None);
    }
}
