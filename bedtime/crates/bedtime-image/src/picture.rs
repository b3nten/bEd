//! An image file as a document: its decoded pixels (`bedtime_ui::image`).
use bedtime_document::DocumentModel;
use bedtime_filesystem::{ByteModel, FileModel};
use bedtime_ui::image::{Decoded, decode};
use std::sync::Arc;

/// An image file's pixels.
#[derive(Clone, Debug, Default)]
pub struct Picture(pub Option<Arc<Decoded>>);

impl PartialEq for Picture {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        }
    }
}

impl DocumentModel for Picture {
    type Change = std::convert::Infallible;
    type OpenState = ();
    fn apply(&mut self, change: &Self::Change) -> Result<Self::Change, String> {
        match *change {}
    }
}

impl ByteModel for Picture {
    fn decode(bytes: &[u8]) -> Result<Self, String> {
        decode(bytes).map(|decoded| Self(Some(Arc::new(decoded))))
    }
    fn encode(&self, _: &mut dyn std::io::Write) -> std::io::Result<()> {
        Err(std::io::Error::other("Images aren't saved"))
    }
}

impl FileModel for Picture {}
