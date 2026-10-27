//! Document models, sources and transaction history, without a host runtime.
//!
//! - `model`: the `DocumentModel` trait every document type implements.
//! - `history`: undo/redo and the commit journal, for any model.
//! - `source`: where documents are loaded from and saved to.
//! - `text`: the text model, `TextBuffer`.
mod history;
mod model;
mod source;
pub mod text;

pub use history::*;
pub use model::DocumentModel;
pub use source::*;
pub use text::{BufferEdit, TextBuffer};
