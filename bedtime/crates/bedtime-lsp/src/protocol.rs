use bedtime_editor::TextBuffer;
use serde::{Deserialize, Serialize};
use std::{
    ops::Range,
    path::{Path, PathBuf},
};

/// Validated absolute, local UTF-8 file URI. No remote authority or query state.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FileUri(String);
impl FileUri {
    pub fn from_path(path: &Path) -> Result<Self, String> {
        if !path.is_absolute() {
            return Err("LSP source path must be absolute".into());
        }
        let text = path.to_str().ok_or("LSP file names must be UTF-8")?;
        if text.contains('\0') {
            return Err("File name contains NUL".into());
        }
        let mut uri = String::from("file://");
        for b in text.bytes() {
            if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
                uri.push(b as char);
            } else {
                use std::fmt::Write;
                write!(uri, "%{b:02X}").unwrap();
            }
        }
        Ok(Self(uri))
    }
    pub fn parse(uri: &str) -> Result<Self, String> {
        let path = uri
            .strip_prefix("file://")
            .filter(|p| p.starts_with('/'))
            .ok_or("Only absolute local file URIs are supported")?;
        if path.contains(['?', '#']) {
            return Err("File URI cannot contain a query or fragment".into());
        }
        let mut bytes = Vec::with_capacity(path.len());
        let mut it = path.bytes();
        while let Some(b) = it.next() {
            if b == b'%' {
                let hex = |b: u8| {
                    (b as char)
                        .to_digit(16)
                        .map(|n| n as u8)
                        .ok_or("Invalid URI escape")
                };
                let a = hex(it.next().ok_or("Incomplete URI escape")?)?;
                let b = hex(it.next().ok_or("Incomplete URI escape")?)?;
                bytes.push(a * 16 + b);
            } else {
                bytes.push(b);
            }
        }
        let path = String::from_utf8(bytes).map_err(|_| "File URI is not UTF-8")?;
        Self::from_path(Path::new(&path))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn path(&self) -> PathBuf {
        // Construction validates escapes and UTF-8 once.
        let source = self.0[7..].as_bytes();
        let mut out = Vec::new();
        let mut at = 0;
        while at < source.len() {
            if source[at] == b'%' {
                let hex = |b: u8| (b as char).to_digit(16).unwrap() as u8;
                out.push(hex(source[at + 1]) * 16 + hex(source[at + 2]));
                at += 3;
            } else {
                out.push(source[at]);
                at += 1;
            }
        }
        PathBuf::from(String::from_utf8(out).unwrap())
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    #[default]
    Utf16,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolRange {
    pub start: Position,
    pub end: Position,
}
impl Encoding {
    pub fn position(self, text: &TextBuffer, byte: usize) -> Result<Position, String> {
        if !text.is_char_boundary(byte) {
            return Err("Position is outside UTF-8 text".into());
        }
        let line = text.line_of(byte);
        let range = text.line_range(line);
        if byte > range.end {
            return Err("Position is inside a line ending".into());
        }
        let character = match self {
            Self::Utf8 => byte - range.start,
            Self::Utf16 => text
                .slice(range.start..byte)
                .chunks()
                .map(|s| s.encode_utf16().count())
                .sum(),
        };
        Ok(Position {
            line: u32::try_from(line).map_err(|_| "Line overflow")?,
            character: u32::try_from(character).map_err(|_| "Column overflow")?,
        })
    }
    pub fn offset(self, text: &TextBuffer, position: Position) -> Result<usize, String> {
        let line = position.line as usize;
        if line >= text.line_count() {
            return Err("LSP line is outside the document".into());
        }
        let range = text.line_range(line);
        let column = position.character as usize;
        if self == Self::Utf8 {
            let byte = range.start + column.min(range.len());
            return text
                .is_char_boundary(byte)
                .then_some(byte)
                .ok_or("LSP column splits UTF-8".into());
        }
        let mut units = 0;
        let mut byte = range.start;
        for chunk in text.slice(range.clone()).chunks() {
            for c in chunk.chars() {
                if units == column {
                    return Ok(byte);
                }
                units += c.len_utf16();
                byte += c.len_utf8();
                if units > column {
                    return Err("LSP column splits a surrogate pair".into());
                }
            }
        }
        Ok(range.end) // Protocol columns beyond EOL are clamped.
    }
    pub fn range(self, text: &TextBuffer, range: ProtocolRange) -> Result<Range<usize>, String> {
        let start = self.offset(text, range.start)?;
        let end = self.offset(text, range.end)?;
        if start > end {
            return Err("LSP range is reversed".into());
        }
        Ok(start..end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn positions_are_protocol_units_not_display_cells() {
        let text = TextBuffer::from("a\t🙂e\u{301}\r\nnext\rfinal\n");
        for encoding in [Encoding::Utf8, Encoding::Utf16] {
            for byte in [0, 1, 2, 6, 7, 9, 11, 15, 16, 21, 22] {
                let p = encoding.position(&text, byte).unwrap();
                assert_eq!(encoding.offset(&text, p).unwrap(), byte);
            }
        }
        assert_eq!(Encoding::Utf16.position(&text, 6).unwrap().character, 4);
        assert!(
            Encoding::Utf16
                .offset(
                    &text,
                    Position {
                        line: 0,
                        character: 3
                    }
                )
                .is_err()
        );
        assert!(
            Encoding::Utf8
                .offset(
                    &text,
                    Position {
                        line: 0,
                        character: 3
                    }
                )
                .is_err()
        );
        assert!(Encoding::Utf16.position(&text, 10).is_err());
    }
    #[test]
    fn local_uri_boundary_round_trips_and_refuses_foreign_sources() {
        let path = Path::new("/tmp/a b🙂%#?.rs");
        let uri = FileUri::from_path(path).unwrap();
        assert_eq!(FileUri::parse(uri.as_str()).unwrap().path(), path);
        for value in [
            "file://host/a",
            "file:///a%",
            "file:///a%00",
            "file:///a#x",
            "ssh:///a",
            "file:///a%ff",
        ] {
            assert!(FileUri::parse(value).is_err(), "{value}");
        }
    }
}
