//! Code editing for workbench apps: the editor (`editor`), tree-sitter
//! analysis (`syntax`) and the Structure panel (`structure`), installed by
//! `CodePlugin`.
pub mod editor;
pub mod structure;
pub mod syntax;

pub use editor::{CodePlugin, EditorPanel};
pub use structure::StructurePanel;
