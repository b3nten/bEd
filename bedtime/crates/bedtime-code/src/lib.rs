//! Code editing for workbench apps. `CodePlugin` registers the editor as the
//! viewer for text files.
pub mod editor;

pub use editor::{CodePlugin, EditorPanel};
