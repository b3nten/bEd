use super::TextBuffer;
use std::ops::Range;

/// A range replacement with cheap, exact snapshot restoration for history.
/// Cached roots are trusted only when the expected root matches by identity.
#[derive(Clone, Debug)]
pub struct BufferEdit {
    pub(crate) range: Range<usize>,
    pub(crate) replacement: TextBuffer,
    restore: Option<(TextBuffer, TextBuffer)>,
}

impl BufferEdit {
    pub fn new(range: Range<usize>, replacement: impl Into<TextBuffer>) -> Self {
        Self {
            range,
            replacement: replacement.into(),
            restore: None,
        }
    }

    pub fn range(&self) -> Range<usize> {
        self.range.clone()
    }

    pub fn replacement(&self) -> &TextBuffer {
        &self.replacement
    }

    pub fn apply(&self, text: &mut TextBuffer) -> Result<Self, String> {
        if self.range.start > self.range.end
            || !text.is_char_boundary(self.range.start)
            || !text.is_char_boundary(self.range.end)
        {
            return Err("Edit range must use valid UTF-8 byte boundaries".into());
        }
        let before = text.clone();
        let existing = text.slice(self.range.clone());
        if existing == self.replacement {
            return Ok(Self {
                range: self.range.clone(),
                replacement: existing,
                restore: Some((before.clone(), before)),
            });
        }
        let removed = if let Some((expected, previous)) = &self.restore {
            if text.ptr_eq(expected) {
                let removed = text.slice(self.range.clone());
                *text = previous.clone();
                removed
            } else {
                text.replace(self.range.clone(), &self.replacement)?
            }
        } else {
            text.replace(self.range.clone(), &self.replacement)?
        };
        Ok(Self {
            range: self.range.start..self.range.start + self.replacement.len(),
            replacement: removed,
            restore: Some((text.clone(), before)),
        })
    }
}
