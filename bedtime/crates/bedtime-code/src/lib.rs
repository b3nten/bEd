//! Code editing for workbench apps: the editor (`editor`), tree-sitter
//! analysis (`syntax`), the Structure panel (`structure`) and the TODOs panel
//! (`todos`), installed by `CodePlugin`.
pub mod editor;
pub mod keymap;
pub mod lsp;
pub mod structure;
pub mod syntax;
pub mod todos;

#[cfg(feature = "persistence")]
pub use editor::{
    AutoCloseBrackets, FormatOnSave, HighlightOccurrences, InsertFinalNewline, LineNumbers,
    TrimTrailingWhitespace, WrapLines,
};
pub use editor::{
    CodePlugin, ComparisonRequest, EditorComparison, EditorDefaults, EditorPanel, ToggleWordWrap,
    draw_editor,
};
pub use structure::{GoToSymbol, ShowStructure, StructurePanel};
pub use todos::{ShowTodos, TodoPanel};
