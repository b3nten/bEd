//! Translated from ned lsp/lsp_locations.h; see LICENSE and NOTICE.
use crate::lsp_uri::LspUri;
use serde_json::Value;
use std::io;
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LspLocation {
    pub file: String,
    pub line: i32,
    pub character: i32,
}
fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid LSP location result")
}
fn position(value: &Value) -> io::Result<(i32, i32)> {
    let coordinate = |name: &str| {
        let number = value
            .get(name)
            .and_then(Value::as_f64)
            .ok_or_else(invalid)?
            .trunc();
        if number.is_finite() && number >= 0.0 && number <= f64::from(u32::MAX) {
            Ok(number as u32 as i32)
        } else {
            Err(invalid())
        }
    };
    Ok((coordinate("line")?, coordinate("character")?))
}
fn range_start(value: &Value) -> io::Result<(i32, i32)> {
    let start = position(value.get("start").ok_or_else(invalid)?)?;
    position(value.get("end").ok_or_else(invalid)?)?;
    Ok(start)
}
fn from_location(value: &Value) -> io::Result<LspLocation> {
    let uri = LspUri::parse(
        value
            .get("uri")
            .and_then(Value::as_str)
            .ok_or_else(invalid)?,
    )?;
    if !uri.is_file_uri() {
        return Err(invalid());
    }
    let (line, character) = range_start(value.get("range").ok_or_else(invalid)?)?;
    Ok(LspLocation {
        file: uri.fs_path(),
        line,
        character,
    })
}
fn from_link(value: &Value) -> io::Result<LspLocation> {
    let uri = LspUri::parse(
        value
            .get("targetUri")
            .and_then(Value::as_str)
            .ok_or_else(invalid)?,
    )?;
    range_start(value.get("targetRange").ok_or_else(invalid)?)?;
    let (line, character) = range_start(value.get("targetSelectionRange").ok_or_else(invalid)?)?;
    Ok(LspLocation {
        file: uri.path().to_owned(),
        line,
        character,
    })
}
pub fn from_definition_result(value: &Value) -> io::Result<Vec<LspLocation>> {
    if value.is_null() {
        return Ok(Vec::new());
    }
    if let Some(array) = value.as_array() {
        let links = array
            .first()
            .is_some_and(|item| item.get("targetUri").is_some());
        array
            .iter()
            .map(if links { from_link } else { from_location })
            .collect()
    } else {
        Ok(vec![from_location(value)?])
    }
}
pub fn from_references_result(value: &Value) -> io::Result<Vec<LspLocation>> {
    if value.is_null() {
        return Ok(Vec::new());
    }
    value
        .as_array()
        .ok_or_else(invalid)?
        .iter()
        .map(from_location)
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn upstream_definition_accepts_normal_location_array() {
        let value = json!([{"uri":"file:///tmp/example.cpp","range":{"start":{"line":3,"character":5},"end":{"line":3,"character":8}}}]);
        let locations = from_definition_result(&value).unwrap();
        assert_eq!(locations.len(), 1);
        assert_eq!(locations[0].line, 3);
        assert_eq!(locations, from_definition_result(&value[0]).unwrap());
        let mut fractional = value[0].clone();
        fractional["range"]["start"]["line"] = json!(3.8);
        assert_eq!(from_definition_result(&fractional).unwrap()[0].line, 3);
    }
    #[test]
    fn definition_links_use_selection_range_and_null_is_an_answer() {
        let range =
            |line| json!({"start":{"line":line,"character":2},"end":{"line":line,"character":9}});
        let value = json!([{"targetUri":"file:///tmp/example.rs","targetRange":range(4),"targetSelectionRange":range(7)}]);
        assert_eq!(from_definition_result(&value).unwrap()[0].line, 7);
        assert!(from_definition_result(&Value::Null).unwrap().is_empty());
        assert!(from_references_result(&json!([])).unwrap().is_empty());
        assert!(from_references_result(&value).is_err());
    }
}
