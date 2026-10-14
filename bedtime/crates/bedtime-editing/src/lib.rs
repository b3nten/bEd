//! Bed core components. Attribution: workspace LICENSE and NOTICE.
pub mod buffer;
pub mod diagnostic;
pub mod document_state;
pub mod editing_events;
pub mod editing_operations;
pub mod folding;
pub mod identity;
pub mod project_undo;
pub mod text_commands;
pub mod text_search;
pub mod text_view_state;
pub mod util;

pub use document_state::DocumentKind;

/// Maximum serialized document size shared by local file and remote transports.
pub const MAX_DOCUMENT_BYTES: usize = 128 * 1024 * 1024;
