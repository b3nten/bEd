//! Document models, sources and transaction history, without a host runtime.
mod history;
mod source;
pub mod text;
pub use history::*;
pub use source::*;
pub use text::{BufferEdit, TextBuffer};

/// What a document is made of. Changes are pure and return their inverse, so
/// history can undo any model the same way.
pub trait DocumentModel: Clone + PartialEq + Send + Sync + 'static {
    type Change: Clone + Send + Sync + 'static;
    fn apply(&mut self, change: &Self::Change) -> Result<Self::Change, String>;
}

impl DocumentModel for TextBuffer {
    type Change = BufferEdit;
    fn apply(&mut self, change: &BufferEdit) -> Result<BufferEdit, String> {
        change.apply(self)
    }
}
