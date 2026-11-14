/// What a document is made of. Changes are pure and return their inverse, so
/// history can undo any model the same way.
pub trait DocumentModel: Clone + PartialEq + Send + Sync + 'static {
    type Change: Clone + Send + Sync + 'static;
    /// What can come with opening it in a viewer: where to put the cursor,
    /// what to select. `()` for models with nothing to point at.
    type OpenState: Default + Clone + Send + Sync + 'static;
    fn apply(&mut self, change: &Self::Change) -> Result<Self::Change, String>;
    /// The state for a line and column (from 0), for callers that only have
    /// those (a terminal's `+open main.rs:42`). Ignored by default.
    fn state_at_line(_line: u32, _column: u32) -> Self::OpenState {
        Self::OpenState::default()
    }
    /// The state selecting from one line and column to another (a search
    /// result). The start's state by default.
    fn state_selecting(from: (u32, u32), _to: (u32, u32)) -> Self::OpenState {
        Self::state_at_line(from.0, from.1)
    }
}

/// A model with a byte form: how bytes become the model and back. Files
/// are kept this way, and documents of one source held by different models
/// (an SVG as text and as an image) pass their content to each other through
/// it.
pub trait ByteModel: DocumentModel {
    /// Read the whole content, refusing bytes that aren't this model (binary
    /// data as text, say).
    fn decode(bytes: &[u8]) -> Result<Self, String>;
    /// Write the model out.
    fn encode(&self, out: &mut dyn std::io::Write) -> std::io::Result<()>;
}

impl ByteModel for crate::TextBuffer {
    fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.contains(&0) {
            return Err("It looks like binary data, not text".into());
        }
        let text = std::str::from_utf8(bytes).map_err(|_| "It isn't UTF-8 text".to_owned())?;
        Ok(crate::TextBuffer::from(text))
    }
    fn encode(&self, out: &mut dyn std::io::Write) -> std::io::Result<()> {
        for chunk in self.chunks() {
            out.write_all(chunk.as_bytes())?;
        }
        Ok(())
    }
}
