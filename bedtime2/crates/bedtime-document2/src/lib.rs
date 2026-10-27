//! Source I/O, editable representations and history without a host runtime.
mod history;
mod source;
pub mod text;
pub use history::*;
pub use source::*;
use std::{io, ops::Range};
pub use text::{BufferEdit, TextBuffer};

/// Changes are pure and return their inverse. Hosts commit isolated candidates.
pub trait DocumentModel: Clone + PartialEq + Send + Sync + 'static {
    type Change: Clone + Send + Sync + 'static;
    fn apply(&mut self, change: &Self::Change) -> Result<Self::Change, String>;
}

/// Byte serialization is a capability, not a requirement on every model.
pub trait ByteDocument: DocumentModel {
    fn decode(bytes: &[u8]) -> io::Result<Self>;
    fn write(&self, output: &mut dyn io::Write) -> io::Result<()>;
    fn open(snapshot: std::sync::Arc<dyn ByteSnapshot>) -> Result<Self, SourceError> {
        if snapshot.len() > 256 * 1024 * 1024 {
            return Err(SourceError::Unsupported(
                "Text exceeds the 256 MiB limit".into(),
            ));
        }
        Self::decode(&snapshot.read_range(0..snapshot.len())?)
            .map_err(|e| SourceError::Unsupported(e.to_string()))
    }
    fn writable() -> bool {
        true
    }
    fn reader(&self) -> io::Result<Box<dyn io::Read + '_>> {
        let mut bytes = Vec::new();
        self.write(&mut bytes)?;
        Ok(Box::new(io::Cursor::new(bytes)))
    }
}

/// The standard UTF-8 representation. It has no viewer or history state.
pub type TextDocument = TextBuffer;
impl DocumentModel for TextBuffer {
    type Change = BufferEdit;
    fn apply(&mut self, change: &BufferEdit) -> Result<BufferEdit, String> {
        change.apply(self)
    }
}
impl ByteDocument for TextBuffer {
    fn decode(bytes: &[u8]) -> io::Result<Self> {
        if bytes.contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Binary source is not text",
            ));
        }
        Ok(Self::from(std::str::from_utf8(bytes).map_err(|e| {
            io::Error::new(io::ErrorKind::InvalidData, e)
        })?))
    }
    fn write(&self, output: &mut dyn io::Write) -> io::Result<()> {
        for chunk in self.chunks() {
            output.write_all(chunk.as_bytes())?;
        }
        Ok(())
    }
    fn reader(&self) -> io::Result<Box<dyn io::Read + '_>> {
        Ok(Box::new(TextReader { text: self, at: 0 }))
    }
}
struct TextReader<'a> {
    text: &'a TextBuffer,
    at: usize,
}
impl io::Read for TextReader<'_> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if self.at == self.text.len() || output.is_empty() {
            return Ok(0);
        }
        let (chunk, base) = self.text.chunk_at(self.at);
        let bytes = &chunk.as_bytes()[self.at - base..];
        let count = output.len().min(bytes.len());
        output[..count].copy_from_slice(&bytes[..count]);
        self.at += count;
        Ok(count)
    }
}
impl ByteDocument for BinaryDocument {
    fn decode(_: &[u8]) -> io::Result<Self> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Binary documents require a version-bound snapshot",
        ))
    }
    fn write(&self, _: &mut dyn io::Write) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Binary documents are read-only",
        ))
    }
    fn open(snapshot: std::sync::Arc<dyn ByteSnapshot>) -> Result<Self, SourceError> {
        Ok(Self { snapshot })
    }
    fn writable() -> bool {
        false
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextChange {
    pub range: Range<usize>,
    pub replacement: String,
}
impl DocumentModel for String {
    type Change = TextChange;
    fn apply(&mut self, change: &TextChange) -> Result<TextChange, String> {
        let range = change.range.clone();
        if range.start > range.end
            || range.end > self.len()
            || !self.is_char_boundary(range.start)
            || !self.is_char_boundary(range.end)
        {
            return Err("Text change is not a valid UTF-8 byte range".into());
        }
        let inverse = TextChange {
            range: range.start..range.start + change.replacement.len(),
            replacement: self[range.clone()].to_owned(),
        };
        self.replace_range(range, &change.replacement);
        Ok(inverse)
    }
}
impl ByteDocument for String {
    fn decode(bytes: &[u8]) -> io::Result<Self> {
        String::from_utf8(bytes.to_vec()).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
    fn write(&self, output: &mut dyn io::Write) -> io::Result<()> {
        output.write_all(self.as_bytes())
    }
}
